use std::cmp::max;
use std::collections::HashMap;

use eyre::Result;
use reqwest::header::ACCEPT;
use serde::Deserialize;

use crate::machine_type::{JobSize, MachineType, System};

#[derive(Deserialize, Debug)]
pub struct MachineTypeStatus {
    runnable: usize,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueueRunnerStatus {
    machine_types: HashMap<MachineType, MachineTypeStatus>,
}

#[derive(Clone, Debug)]
pub struct HardwarePlan {
    pub bid: f64,
    pub plan: String,
    pub netboot_url: String,
}

#[derive(Clone, Debug)]
pub struct HardwareCategory {
    pub divisor: usize,
    #[allow(dead_code)] // "field `minimum` is never read"
    pub minimum: usize,
    pub plans: Vec<HardwarePlan>,
}

pub async fn get_desired_hardware(http_client: &reqwest::Client) -> Result<Vec<HardwarePlan>> {
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
