use anyhow::Result;
use bollard::models::{PortBinding, RestartPolicy, RestartPolicyNameEnum};
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::HashMap, fmt, path::Path};
use tokio::fs::read_to_string;

#[derive(Debug)]
pub struct Config {
    pub name: String,
    pub url: String,
    pub restart: Option<RestartPolicy>,
    pub env: Option<Vec<String>>,
    pub volumes: Option<Vec<String>>,
    pub ports: Option<HashMap<String, Option<Vec<PortBinding>>>>,
}

impl Config {
    pub async fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        async fn inner(path: &Path) -> Result<Config> {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let config: ConfigInner = toml::from_str(&read_to_string(path).await?)?;
            Ok(Config {
                name,
                url: config.url,
                restart: config.restart,
                env: config.env,
                volumes: config.volumes,
                ports: config.ports,
            })
        }
        inner(path.as_ref()).await
    }
}

#[derive(Debug)]
struct ConfigInner {
    url: String,
    restart: Option<RestartPolicy>,
    env: Option<Vec<String>>,
    volumes: Option<Vec<String>>,
    ports: Option<HashMap<String, Option<Vec<PortBinding>>>>,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum ConfigInnerField {
    Url,
    Restart,
    Env,
    Volumes,
    Ports,
}

impl<'de> Deserialize<'de> for ConfigInner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ConfigInnerVisitor;

        impl<'de> Visitor<'de> for ConfigInnerVisitor {
            type Value = ConfigInner;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct ConfigInner")
            }

            fn visit_map<V>(self, mut map: V) -> Result<ConfigInner, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut url = None;
                let mut restart = None;
                let mut env = None;
                let mut volumes = None;
                let mut ports = None;
                loop {
                    if let Ok(key_opt) = map.next_key() {
                        if let Some(key) = key_opt {
                            match key {
                                ConfigInnerField::Url => {
                                    if url.is_some() {
                                        return Err(de::Error::duplicate_field("url"));
                                    }
                                    url = Some(map.next_value()?);
                                }
                                ConfigInnerField::Restart => {
                                    if restart.is_some() {
                                        return Err(de::Error::duplicate_field("restart"));
                                    }
                                    let rp: Option<&str> = map.next_value()?;
                                    restart = Some(RestartPolicy {
                                        name: rp.map(|rst| match rst {
                                            "no" => RestartPolicyNameEnum::NO,
                                            "always" => RestartPolicyNameEnum::ALWAYS,
                                            "on-failure" => RestartPolicyNameEnum::ON_FAILURE,
                                            "unless-stopped" => {
                                                RestartPolicyNameEnum::UNLESS_STOPPED
                                            }
                                            _ => RestartPolicyNameEnum::EMPTY,
                                        }),
                                        ..Default::default()
                                    });
                                }
                                ConfigInnerField::Env => {
                                    if env.is_some() {
                                        return Err(de::Error::duplicate_field("env"));
                                    }
                                    let e: Option<HashMap<String, String>> = map.next_value()?;
                                    env = e.map(|vars| {
                                        vars.iter().map(|(k, v)| [k, "=", v].concat()).collect()
                                    });
                                }
                                ConfigInnerField::Volumes => {
                                    if volumes.is_some() {
                                        return Err(de::Error::duplicate_field("volumes"));
                                    }
                                    let v: Option<HashMap<String, String>> = map.next_value()?;
                                    volumes = v.map(|vars| {
                                        vars.iter().map(|(k, v)| [k, ":", v].concat()).collect()
                                    });
                                }
                                ConfigInnerField::Ports => {
                                    if ports.is_some() {
                                        return Err(de::Error::duplicate_field("ports"));
                                    }
                                    let p: Option<HashMap<String, [String; 2]>> =
                                        map.next_value()?;
                                    ports = p.map(|p| {
                                        let mut ports = HashMap::new();
                                        p.iter().for_each(|(k, v)| {
                                            ports
                                                .entry(k.clone())
                                                .or_insert_with(|| Some(Vec::new()))
                                                .as_mut()
                                                .and_then(|p| {
                                                    p.push(PortBinding {
                                                        host_ip: Some(v[0].clone()),
                                                        host_port: Some(v[1].clone()),
                                                    });
                                                    Some(p)
                                                });
                                        });
                                        ports
                                    });
                                }
                            }
                        } else {
                            break;
                        }
                    }
                }

                let url = url.ok_or_else(|| de::Error::missing_field("url"))?;
                Ok(ConfigInner {
                    url,
                    restart,
                    env,
                    volumes,
                    ports,
                })
            }
        }

        const FIELDS: &[&str] = &["url", "restart", "env", "volumes"];
        deserializer.deserialize_struct("ConfigInner", FIELDS, ConfigInnerVisitor)
    }
}
