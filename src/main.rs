use std::cmp::max;
use std::collections::HashMap;

use eyre::Result;
use reqwest::header::ACCEPT;
use serde::Deserialize;
use time::OffsetDateTime;

mod device;
mod machine_type;

#[derive(Clone, Debug)]
pub struct HardwarePlan {
    bid: f64,
    plan: String,
    netboot_url: String,
}

#[derive(Deserialize, Debug)]
pub struct MachineTypeStatus {
    runnable: usize,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueueRunnerStatus {
    machine_types: HashMap<machine_type::MachineType, MachineTypeStatus>,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct System(String);

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum JobSize {
    Small,
    BigParallel,
}

#[derive(Clone, Debug)]
pub struct HardwareCategory {
    divisor: usize,
    #[allow(dead_code)] // "field `minimum` is never read"
    minimum: usize,
    plans: Vec<HardwarePlan>,
}

async fn get_desired_hardware(http_client: &reqwest::Client) -> Result<Vec<HardwarePlan>> {
    let hardware_map: HashMap<(System, JobSize), HardwareCategory> = HashMap::from([
        (
            (System("aarch64-linux".into()), JobSize::Small),
            HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![
                    HardwarePlan{
                        bid: 2.0,
                        plan: "c3.large.arm64".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-large-arm".into(),
                    },
                ]
            }
        ),
        (
            (System("aarch64-linux".into()), JobSize::BigParallel),
            HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![
                    HardwarePlan{
                        bid: 2.0,
                        plan: "c3.large.arm64".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-large-arm--big-parallel".into(),
                    },
                ]
            }
        ),
        (
            (System("x86_64-linux".into()), JobSize::Small),
            HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![
                    HardwarePlan{
                        bid: 2.0,
                        plan: "c3.medium.x86".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-medium-x86".into(),
                    },
                    HardwarePlan{
                        bid: 2.0,
                        plan: "m3.large.x86".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/m3-large-x86".into(),
                    }
                ]
            }
        ),
        (
            (System("x86_64-linux".into()), JobSize::BigParallel),
            HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![
                    HardwarePlan{
                        bid: 2.0,
                        plan: "c3.medium.x86".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-medium-x86--big-parallel".into(),
                    },
                    HardwarePlan{
                        bid: 2.0,
                        plan: "m3.large.x86".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/m3-large-x86--big-parallel".into(),
                    }
                ]
            }
        )
    ]);

    let status = http_client
        .get("https://hydra.nixos.org/queue-runner-status")
        .header(ACCEPT, "application/json")
        .send()
        .await?
        .json::<QueueRunnerStatus>()
        .await?;

    let mut buckets: HashMap<System, HashMap<JobSize, usize>> = HashMap::from([
        (System("aarch64-linux".into()), HashMap::new()),
        (System("x86_64-linux".into()), HashMap::new()),
    ]);

    for (key, status) in status.machine_types.iter() {
        if let Some(bucket) = buckets.get_mut(&key.system()) {
            *bucket.entry(key.get_job_size()).or_default() += status.runnable;
        }
    }

    println!("Work summary:");
    for (system, sizes) in buckets.iter() {
        for (size, runnable) in sizes.iter() {
            println!("{:?} {:?} = {}", system, size, runnable);
        }
    }

    // Decide how many machines we need to make
    let mut desired_hardware: Vec<HardwarePlan> = vec![];
    for (system, sizes) in buckets.iter() {
        for (size, runnable) in sizes.iter() {
            if let Some(category) = hardware_map.get(&(system.clone(), size.clone())) {
                let wanted = max(1, runnable / category.divisor);
                if category.plans.is_empty() {
                    println!(
                        "WARNING: {:?}/{:?}'s hardwarecategory is has no plans",
                        system, size
                    );

                    continue;
                }

                desired_hardware.extend(category.plans.iter().cycle().take(wanted).cloned());
            } else {
                println!(
                    "WARNING: {:?}/{:?} has no hardwarecategory is the hardware map",
                    system, size
                );
            }
        }
    }

    Ok(desired_hardware)
}

#[tokio::main]
async fn main() -> Result<()> {
    let equinix_auth_token =
        std::env::var("METAL_AUTH_TOKEN").expect("Please set METAL_AUTH_TOKEN");
    let equinix_project_id =
        std::env::var("METAL_PROJECT_ID").expect("Please set METAL_PROJECT_ID");

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

    let mut desired_hardware = get_desired_hardware(&http_client).await?;

    let mut all_devices: Vec<device::Device> =
        device::get_all_devices(&http_client, &equinix_auth_token, &equinix_project_id)
            .await?
            .into_iter()
            .filter(|device| {
                device
                    .tags
                    .contains(&"terraform-packet-nix-builder".to_string())
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
        if let Some(idx) = desired_hardware.iter().position(|desired| {
            Some(&desired.netboot_url) == device.ipxe_script_url.as_ref()
                && desired.plan == device.plan.class
        }) {
            desired_hardware.swap_remove(idx);
            to_keep.push(device);
        } else {
            to_delete.push(device);
        }
    }

    for desired in desired_hardware.iter() {
        println!("Creating: {:#?}", desired);
        device::create_device(
            &http_client,
            &equinix_auth_token,
            &equinix_project_id,
            desired.clone(),
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
            device::get_current_jobs(&http_client, device).await?
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
        let jobs = device::get_current_jobs(&http_client, dev).await?;

        println!(
            "-{} {} jobs {} {:?}",
            dev.short_id, jobs, dev.plan.class, dev.ipxe_script_url
        );
    }
    for dev in to_keep.iter() {
        let jobs = device::get_current_jobs(&http_client, dev).await?;

        println!(
            " {} {} jobs {} {:?}",
            dev.short_id, jobs, dev.plan.class, dev.ipxe_script_url
        );
    }
    for dev in desired_hardware.iter() {
        println!("+-------- 0 jobs {} {:?}", dev.plan, dev.netboot_url);
    }

    Ok(())
}
