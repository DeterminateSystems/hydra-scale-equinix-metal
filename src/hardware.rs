use std::cmp::max;
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
    pub divisor: usize,
    #[allow(dead_code)] // "field `minimum` is never read"
    pub minimum: usize,
    pub plans: Vec<HardwarePlan>,
}

type CategoryMap = HashMap<System, HashMap<JobSize, HardwareCategory>>;

#[derive(Deserialize)]
struct Config {
    category: CategoryMap,
}

pub fn parse_categories_file(file: &Path) -> Result<CategoryMap> {
    let json_str = std::fs::read_to_string(file)?;
    let config: Config = serde_json::from_str(&json_str)?;

    Ok(config.category)
}

pub async fn get_desired_hardware(
    http_client: &reqwest::Client,
    hydra_root: &str,
    categories_file: &Path,
) -> Result<Vec<HardwarePlan>> {
    let categories = parse_categories_file(categories_file)?;
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
            if let Some(category) = categories.get(&system).and_then(|e| e.get(&size)) {
                let wanted = max(1, runnable / category.divisor);
                if category.plans.is_empty() {
                    println!(
                        "WARNING: {:?}/{:?}'s hardwarecategory has no plans",
                        system, size
                    );

                    continue;
                }

                desired_hardware.extend(category.plans.iter().cycle().take(wanted).cloned());
            } else {
                println!(
                    "WARNING: {:?}/{:?} has no hardwarecategory in the hardware map",
                    system, size
                );
            }
        }
    }

    Ok(desired_hardware)
}
