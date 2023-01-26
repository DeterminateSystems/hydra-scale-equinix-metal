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

fn get_hardware_category(system: System, job_size: JobSize) -> Option<HardwareCategory> {
    let category = match (system, job_size) {
        (System(system), JobSize::Small) => match system.as_ref() {
            "aarch64-linux" => HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![HardwarePlan {
                    bid: 2.0,
                    plan: "c3.large.arm64".into(),
                    netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-large-arm".into(),
                }],
            },
            "x86_64-linux" => HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![
                    HardwarePlan {
                        bid: 2.0,
                        plan: "c3.medium.x86".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-medium-x86".into(),
                    },
                    HardwarePlan {
                        bid: 2.0,
                        plan: "m3.large.x86".into(),
                        netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/m3-large-x86".into(),
                    },
                ],
            },
            _ => return None,
        },
        (System(system), JobSize::BigParallel) => match system.as_ref() {
            "aarch64-linux" => HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![HardwarePlan {
                    bid: 2.0,
                    plan: "c3.large.arm64".into(),
                    netboot_url: "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-large-arm--big-parallel"
                        .into(),
                }],
            },
            "x86_64-linux" => HardwareCategory {
                divisor: 2000,
                minimum: 1,
                plans: vec![
                    HardwarePlan {
                        bid: 2.0,
                        plan: "c3.medium.x86".into(),
                        netboot_url:
                            "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/c3-medium-x86--big-parallel".into(),
                    },
                    HardwarePlan {
                        bid: 2.0,
                        plan: "m3.large.x86".into(),
                        netboot_url:
                            "https://netboot.nixos.org/dispatch/hydra/hydra.nixos.org/equinix-metal-builders/main/m3-large-x86--big-parallel".into(),
                    },
                ],
            },
            _ => return None,
        },
    };

    Some(category)
}

pub async fn get_desired_hardware(
    http_client: &reqwest::Client,
    hydra_root: &str,
) -> Result<Vec<HardwarePlan>> {
    let status = http_client
        .get(format!("{hydra_root}/queue-runner-status"))
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
            if let Some(category) = get_hardware_category(system.clone(), size.clone()) {
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
