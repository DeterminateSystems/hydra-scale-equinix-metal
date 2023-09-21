use std::cmp::{max, min};
use std::collections::HashMap;
use std::path::Path;

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

#[derive(Clone, Debug, Deserialize)]
pub struct HardwarePlan {
    pub bid: f64,
    pub plan: String,
    pub netboot_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HardwareCategory {
    pub size: JobSize,
    pub divisor: usize,
    pub minimum: usize,
    pub maximum: usize,
    pub plans: Vec<HardwarePlan>,
}

type CategoryMap = HashMap<System, Vec<HardwareCategory>>;

#[derive(Deserialize)]
pub struct Config {
    categories: CategoryMap,
    tags: Vec<String>,
    facilities: Vec<String>,
}

#[derive(Debug)]
pub struct DesiredHardwareConfig {
    pub plans: Vec<HardwarePlan>,
    pub tags: Vec<String>,
    pub facilities: Vec<String>,
}

pub fn parse_config_file(file: &Path) -> Result<Config> {
    let json_str = std::fs::read_to_string(file)?;
    let config: Config = serde_json::from_str(&json_str)?;

    Ok(config)
}

pub async fn get_desired_hardware(
    http_client: &reqwest::Client,
    hydra_root: &str,
    config_file: &Path,
) -> Result<DesiredHardwareConfig> {
    let config = parse_config_file(config_file)?;
    let categories = config.categories;
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
            if let Some(size_categories) = categories
                .get(system)
                .and_then(|cats| Some(cats.iter().filter(|cat| &cat.size == size)))
            {
                for category in size_categories {
                    if category.plans.is_empty() {
                        println!(
                            "WARNING: {:?}/{:?}'s hardwarecategory has no plans",
                            system, size
                        );
                    } else {
                        let wanted = min(
                            category.maximum,
                            max(category.minimum, runnable / category.divisor),
                        );
                        desired_hardware
                            .extend(category.plans.iter().cycle().take(wanted).cloned());
                    }
                }
            } else {
                println!(
                    "WARNING: {:?}/{:?} has no hardwarecategory in the hardware map",
                    system, size
                );
            }
        }
    }

    let mut tags = config.tags;
    tags.dedup();
    let mut facilities = config.facilities;
    facilities.dedup();

    Ok(DesiredHardwareConfig {
        plans: desired_hardware,
        tags,
        facilities,
    })
}
