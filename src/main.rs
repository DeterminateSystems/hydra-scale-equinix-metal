use eyre::{eyre, Result, WrapErr};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::cmp::max;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct QueueRunnerStatus {
    machine_types: HashMap<MachineType, MachineTypeStatus>,
}

#[derive(Deserialize, Debug, Hash, Eq, PartialEq)]
struct MachineType(String);
impl MachineType {
    fn system(&self) -> System {
        System(self.0.split(":").next().unwrap().to_string())
    }

    fn features(&self) -> Vec<Feature> {
        self.0
            .split(":")
            .skip(1)
            .next()
            .unwrap_or("")
            .split(",")
            .filter(|x| *x != "")
            .map(|x| x.to_string())
            .map(Feature)
            .collect()
    }

    fn get_job_size(&self) -> JobSize {
        if self.features().contains(&Feature("big-parallel".into())) {
            return JobSize::BigParallel;
        } else {
            return JobSize::Small;
        }
    }
}
#[cfg(test)]
mod machinetype_tests {
    use super::*;

    #[test]
    fn test_empty() {
        let mt = MachineType("".to_string());
        assert_eq!(mt.system(), System("".to_string()));
        assert_eq!(mt.features(), vec![]);
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct System(String);

#[derive(Debug, Eq, PartialEq)]
struct Feature(String);

#[derive(Deserialize, Debug)]
struct MachineTypeStatus {
    runnable: usize,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
enum JobSize {
    Small,
    BigParallel,
}

#[derive(Clone, Debug)]
struct HardwareCategory {
    divisor: usize,
    minimum: usize,
    plans: Vec<HardwarePlan>,
}

#[derive(Clone, Debug)]
struct HardwarePlan {
    bid: f64,
    plan: String,
    netboot_url: String,
}

#[derive(Deserialize, Debug)]
struct ResponseReference {
    href: String,
}

#[derive(Deserialize, Debug)]
struct ResponseMeta {
    next: Option<ResponseReference>,
}

#[derive(Deserialize, Debug)]
struct DeviceList {
    devices: Vec<Device>,
    meta: ResponseMeta,
}

#[derive(Deserialize, Debug)]
struct Device {
    hostname: String,
    id: String,
    short_id: String,
    #[serde(with = "time::serde::iso8601")]
    created_at: OffsetDateTime,
    device_type: DeviceType,
    state: DeviceState,
    ipxe_script_url: Option<String>,
    #[serde(default)]
    spot_instance: bool,
    plan: Plan,
    tags: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Plan {
    class: String,
}

#[derive(Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum DeviceType {
    SpotInstance,
    OnDemand,
}

#[derive(Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum DeviceState {
    Provisioning,
    Active,
    Queued,
}

#[derive(Serialize, Debug)]
struct CreateDeviceRequest {
    always_pxe: bool,
    facility: Vec<String>,
    hostname: String,
    ipxe_script_url: String,
    operating_system: String,
    plan: String,
    spot_instance: bool,
    spot_price_max: f64,
    tags: Vec<String>,
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

    let mut all_devices: Vec<Device> =
        get_all_devices(&http_client, &equinix_auth_token, &equinix_project_id)
            .await?
            .into_iter()
            .filter(|device| {
                device
                    .tags
                    .contains(&"terraform-packet-nix-builder".to_string())
            })
            .filter(|device| device.device_type == DeviceType::SpotInstance)
            .collect();

    let mut to_delete: Vec<Device>;

    // Take out all the old devices that we want to cycle out anyway,
    // and devices which are already in drain
    (to_delete, all_devices) = all_devices.into_iter().partition(|device| {
        (device.created_at < older_than) || device.tags.contains(&"skip-hydra".to_string())
    });

    let mut to_keep: Vec<Device> = vec![];
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
        create_device(
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

            add_device_tag(&http_client, &equinix_auth_token, &device, tags).await?;
        }
    }

    for device in to_delete.iter() {
        let jobs = if device.created_at < urgently_terminate {
            println!("Disregarding the device's in progress jobs: it has exceeded the urgent termination date");
            0
        } else {
            get_current_jobs(&http_client, &device).await?
        };

        if jobs == 0 {
            if device.state != DeviceState::Active {
                println!("Would destroy but it isn't active ({:?})", device.state);
            } else {
                println!("Destroying...");
                destroy_device(&http_client, &equinix_auth_token, &device).await?;
            }
        }
    }

    for dev in to_delete.iter() {
        let jobs = get_current_jobs(&http_client, &dev).await?;

        println!(
            "-{} {} jobs {} {:?}",
            dev.short_id, jobs, dev.plan.class, dev.ipxe_script_url
        );
    }
    for dev in to_keep.iter() {
        let jobs = get_current_jobs(&http_client, &dev).await?;

        println!(
            " {} {} jobs {} {:?}",
            dev.short_id, jobs, dev.plan.class, dev.ipxe_script_url
        );
    }
    for dev in desired_hardware.iter() {
        println!("+-------- 0 jobs {} {:?}", dev.plan, dev.netboot_url);
    }

    return Ok(());
}

async fn get_current_jobs(http_client: &reqwest::Client, device: &Device) -> Result<u64> {
    let url = format!(
        "https://status.nixos.org/prometheus/api/v1/query?query=hydra_machine_current_jobs{{host=%22root@{shortid}.packethost.net%22}}",
        shortid=device.short_id
    );

    let raw = http_client
        .get(&url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let getval = |resp: &serde_json::value::Value| -> Result<u64> {
        let result = resp
            .get("data")
            .ok_or(eyre!("no .data"))?
            .get("result")
            .ok_or(eyre!("no .data.result"))?;

        if let Some(measurement) = result.get(0) {
            measurement
                .get("value")
                .ok_or(eyre!("no .data.result[0].value"))?
                .get(1)
                .ok_or(eyre!("no .data.result[0].value.1"))?
                .as_str()
                .ok_or(eyre!("not a string: .data.result[0].value.1"))?
                .parse()
                .wrap_err("couldn't convert .data.result[0].value.1 to a u64")
        } else {
            Ok(0)
        }
    };

    getval(&raw).wrap_err_with(|| {
        format!(
            "failed to parse json from {}, here's the raw content: {:#?}",
            url, raw
        )
    })
}

async fn create_device(
    http_client: &reqwest::Client,
    equinix_auth_token: &str,
    equinix_project_id: &str,
    plan: HardwarePlan,
) -> Result<Device> {
    let raw = http_client
        .post(format!(
            "https://api.equinix.com/metal/v1/projects/{}/devices",
            equinix_project_id
        ))
        .json(&CreateDeviceRequest {
            always_pxe: true,
            hostname: plan.plan.clone(),
            ipxe_script_url: plan.netboot_url.into(),
            operating_system: "custom_ipxe".into(),
            plan: plan.plan,
            spot_instance: true,
            spot_price_max: plan.bid,
            tags: vec![
                "terraform-packet-nix-builder".to_string(),
                "hydra".to_string(),
            ],
            facility: [
                "am6", "da11", "da6", "dc10", "dc13", "fr2", "fr8", "la4", "ny5", "ny7", "se4",
                "sv15", "sv16",
            ]
            .into_iter()
            .map(|x| x.to_owned())
            .collect(),
        })
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header("X-Auth-Token", equinix_auth_token)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    serde_json::from_str(&raw.to_string())
        .wrap_err_with(|| format!("failed to parse json, here's the raw content: {:#?}", raw))
}

async fn add_device_tag(
    http_client: &reqwest::Client,
    equinix_auth_token: &str,
    device: &Device,
    tags: Vec<String>,
) -> Result<Device> {
    let raw = http_client
        .put(format!(
            "https://api.equinix.com/metal/v1/devices/{}",
            device.id
        ))
        .json(&HashMap::from([("tags", tags)]))
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header("X-Auth-Token", equinix_auth_token)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    serde_json::from_str(&raw.to_string())
        .wrap_err_with(|| format!("failed to parse json, here's the raw content: {:#?}", raw))
}

async fn destroy_device(
    http_client: &reqwest::Client,
    equinix_auth_token: &str,
    device: &Device,
) -> Result<()> {
    let raw = http_client
        .delete(format!(
            "https://api.equinix.com/metal/v1/devices/{}",
            device.id
        ))
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header("X-Auth-Token", equinix_auth_token)
        .send()
        .await?;

    if raw.status().is_success() {
        return Ok(());
    } else {
        return Err(eyre!(raw.json::<serde_json::Value>().await?));
    }
}

async fn get_all_devices(
    http_client: &reqwest::Client,
    equinix_auth_token: &str,
    equinix_project_id: &str,
) -> Result<Vec<Device>> {
    let mut all_devices: Vec<Device> = vec![];

    let mut next_url = Some(format!(
        "https://api.equinix.com/metal/v1/projects/{}/devices",
        equinix_project_id
    ));
    while let Some(url) = next_url {
        let raw = http_client
            .get(url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Auth-Token", equinix_auth_token)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let devices: DeviceList = serde_json::from_str(&raw.to_string()).wrap_err_with(|| {
            format!("failed to parse json, here's the raw content: {:#?}", raw)
        })?;

        next_url = devices
            .meta
            .next
            .map(|respref| format!("https://api.equinix.com/metal/v1{}", respref.href));
        all_devices.extend(devices.devices);
    }

    return Ok(all_devices);
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

    return Ok(desired_hardware);
}
