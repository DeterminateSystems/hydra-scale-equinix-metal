use serde::Deserialize;

use crate::{JobSize, System};

#[derive(Debug, Eq, PartialEq)]
pub struct Feature(String);

#[derive(Deserialize, Debug, Hash, Eq, PartialEq)]
pub struct MachineType(String);

impl MachineType {
    pub fn system(&self) -> System {
        System(self.0.split(":").next().unwrap().to_string())
    }

    pub fn features(&self) -> Vec<Feature> {
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

    pub fn get_job_size(&self) -> JobSize {
        if self.features().contains(&Feature("big-parallel".into())) {
            return JobSize::BigParallel;
        } else {
            return JobSize::Small;
        }
    }
}

#[cfg(test)]
pub mod machinetype_tests {
    use super::*;

    #[test]
    fn test_empty() {
        let mt = MachineType("".to_string());
        assert_eq!(mt.system(), System("".to_string()));
        assert_eq!(mt.features(), vec![]);
    }
}
