use std::{collections::BTreeSet, path::PathBuf};

use clap::Parser;
use eyre::Result;
use time::OffsetDateTime;

mod device;
mod hardware;
mod machine_type;

/// A tool for providing autoscaling for a Hydra instance via Equinix Metal.
#[derive(Parser, Debug)]
#[clap(author, version)]
struct Cli {
    /// The root of the Hydra instance used as a basis for autoscaling.
    #[clap(long, default_value = "https://hydra.nixos.org")]
    hydra_root: String,

    /// The root of the Prometheus server that contains information about Hydra machines.
    #[clap(long, default_value = "https://status.nixos.org/prometheus")]
    prometheus_root: String,

    /// A JSON description of machines and their Nix system types and job sizes, and the tags and
    /// metro with which to create the machines.
    #[clap(long, required = true)]
    config_file: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let equinix_auth_token =
        std::env::var("METAL_AUTH_TOKEN").expect("Please set METAL_AUTH_TOKEN");
    let equinix_project_id =
        std::env::var("METAL_PROJECT_ID").expect("Please set METAL_PROJECT_ID");

    real_main(
        equinix_auth_token,
        equinix_project_id,
        args.hydra_root,
        args.prometheus_root,
        args.config_file,
    )
    .await
}

async fn real_main(
    equinix_auth_token: String,
    equinix_project_id: String,
    hydra_root: String,
    prometheus_root: String,
    config_file: PathBuf,
) -> Result<()> {
    let older_than = OffsetDateTime::now_utc() - time::Duration::DAY;
    let urgently_terminate = older_than - time::Duration::DAY;

    let http_client = reqwest::Client::new();
    /*
        curl \
        --header 'Accept: application/json' \
        --header 'Content-Type: application/json' \
        --header "X-Auth-Token: $PACKET_AUTH_TOKEN" \
        --fail \
        "https://api.equinix.com/metal/v1/spot-market-requests/${1}?include=devices" \
        | jq -c '.devices[] | { id, short_id }'
    */

    let mut desired_hardware =
        hardware::get_desired_hardware(&http_client, &hydra_root, &config_file).await?;
    let desired_tags = BTreeSet::from_iter(desired_hardware.tags.iter());

    let mut all_devices: Vec<device::Device> =
        device::get_all_devices(&http_client, &equinix_auth_token, &equinix_project_id)
            .await?
            .into_iter()
            .filter(|device| {
                let device_tags = BTreeSet::from_iter(device.tags.iter());
                device_tags.is_superset(&desired_tags)
            })
            .filter(|device| device.device_type == device::DeviceType::SpotInstance)
            .collect();

    let mut to_delete: Vec<device::Device>;

    // Take out all the old devices that we want to cycle out anyway,
    // and devices which are already in drain
    (to_delete, all_devices) = all_devices.into_iter().partition(|device| {
        (device.created_at < older_than) || device.tags.contains(&"skip-hydra".to_string())
    });

    let mut to_keep: Vec<device::Device> = vec![];
    for device in all_devices.into_iter() {
        // See if desired_hardware has a matching device
        if let Some(idx) = desired_hardware.plans.iter().position(|desired| {
            Some(&desired.netboot_url) == device.ipxe_script_url.as_ref()
                && desired.plan == device.plan.class
        }) {
            desired_hardware.plans.swap_remove(idx);
            to_keep.push(device);
        } else {
            to_delete.push(device);
        }
    }

    for desired in desired_hardware.plans.iter() {
        println!("Creating: {:#?}", desired);
        device::create_device(
            &http_client,
            &equinix_auth_token,
            &equinix_project_id,
            desired.clone(),
            &desired_hardware.tags,
            &desired_hardware.metro,
        )
        .await?;
    }

    for device in to_delete.iter() {
        if !device.tags.contains(&"skip-hydra".to_string()) {
            println!("Giving {} a skip-hydra tag", device.id);
            let mut tags = device.tags.clone();
            tags.push("skip-hydra".to_string());

            device::add_device_tag(&http_client, &equinix_auth_token, device, tags).await?;
        }
    }

    for device in to_delete.iter() {
        let jobs = if device.created_at < urgently_terminate {
            println!("Disregarding the device's in progress jobs: it has exceeded the urgent termination date");
            0
        } else {
            device::get_current_jobs(&http_client, device, &prometheus_root).await?
        };

        if jobs == 0 {
            if device.state != device::DeviceState::Active {
                println!("Would destroy but it isn't active ({:?})", device.state);
            } else {
                println!("Destroying...");
                device::destroy_device(&http_client, &equinix_auth_token, device).await?;
            }
        }
    }

    for dev in to_delete.iter() {
        let jobs = device::get_current_jobs(&http_client, dev, &prometheus_root).await?;

        println!(
            "-{} {} jobs {} {:?}",
            dev.short_id, jobs, dev.plan.class, dev.ipxe_script_url
        );
    }
    for dev in to_keep.iter() {
        let jobs = device::get_current_jobs(&http_client, dev, &prometheus_root).await?;

        println!(
            " {} {} jobs {} {:?}",
            dev.short_id, jobs, dev.plan.class, dev.ipxe_script_url
        );
    }
    for dev in desired_hardware.plans.iter() {
        println!("+-------- 0 jobs {} {:?}", dev.plan, dev.netboot_url);
    }

    Ok(())
}
