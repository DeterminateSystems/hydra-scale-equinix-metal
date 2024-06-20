use std::collections::HashMap;

use eyre::{eyre, Result, WrapErr};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::hardware::HardwarePlan;

#[derive(Deserialize, Debug)]
pub struct Plan {
    pub class: String,
}

#[derive(Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    SpotInstance,
    OnDemand,
}

#[derive(Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Provisioning,
    Active,
    Queued,
}

#[derive(Deserialize, Debug)]
pub struct Device {
    pub hostname: String,
    pub id: String,
    pub short_id: String,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    pub device_type: DeviceType,
    pub state: DeviceState,
    pub ipxe_script_url: Option<String>,
    #[serde(default)]
    pub spot_instance: bool,
    pub plan: Plan,
    pub tags: Vec<String>,
}

#[derive(Serialize, Debug)]
struct CreateDeviceRequest {
    always_pxe: bool,
    metro: String,
    hostname: String,
    ipxe_script_url: String,
    operating_system: String,
    plan: String,
    spot_instance: bool,
    spot_price_max: f64,
    tags: Vec<String>,
}

pub async fn create_device(
    http_client: &reqwest::Client,
    equinix_auth_token: &str,
    equinix_project_id: &str,
    plan: HardwarePlan,
    tags: &[String],
    metro: &str,
) -> Result<Device> {
    let raw = http_client
        .post(format!(
            "https://api.equinix.com/metal/v1/projects/{}/devices",
            equinix_project_id
        ))
        .json(&CreateDeviceRequest {
            always_pxe: true,
            hostname: plan.plan.clone(),
            ipxe_script_url: plan.netboot_url,
            operating_system: "custom_ipxe".into(),
            plan: plan.plan,
            spot_instance: true,
            spot_price_max: plan.bid,
            tags: tags.to_vec(),
            metro: metro.into(),
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

pub async fn add_device_tag(
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

pub async fn destroy_device(
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
        Ok(())
    } else {
        Err(eyre!(raw.json::<serde_json::Value>().await?))
    }
}

pub async fn get_current_jobs(
    http_client: &reqwest::Client,
    device: &Device,
    prometheus_root: &str,
) -> Result<u64> {
    let url = format!(
        "{prometheus_root}/api/v1/query?query=hydra_machine_current_jobs{{host=%22root@{shortid}.packethost.net%22}}",
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
    pub devices: Vec<Device>,
    meta: ResponseMeta,
}

pub async fn get_all_devices(
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

    Ok(all_devices)
}
