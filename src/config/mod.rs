use std::{
    collections::BTreeMap,
    fmt,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};

use figment::Figment;
use ruma::{OwnedServerName, RoomVersionId};
use serde::{de::IgnoredAny, Deserialize};
use tracing::{error, warn};

mod proxy;

use self::proxy::ProxyConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_address")]
    pub address: IpAddr,
    #[serde(default = "default_port")]
    pub port: u16,
    pub tls: Option<TlsConfig>,
    pub unix_socket_path: Option<PathBuf>,
    #[serde(default = "default_unix_socket_perms")]
    pub unix_socket_perms: u32,
    pub server_name: OwnedServerName,
    #[serde(default = "default_database_backend")]
    pub database_backend: String,
    pub database_path: String,
    #[serde(default = "default_db_cache_capacity_mb")]
    pub db_cache_capacity_mb: f64,
    #[serde(default = "true_fn")]
    pub enable_lightning_bolt: bool,
    #[serde(default = "true_fn")]
    pub allow_check_for_updates: bool,
    #[serde(default = "default_conduit_cache_capacity_modifier")]
    pub conduit_cache_capacity_modifier: f64,
    #[serde(default = "default_rocksdb_max_open_files")]
    pub rocksdb_max_open_files: i32,
    #[serde(default = "default_pdu_cache_capacity")]
    pub pdu_cache_capacity: u32,
    #[serde(default = "default_cleanup_second_interval")]
    pub cleanup_second_interval: u32,
    #[serde(default = "default_max_request_size")]
    pub max_request_size: u32,
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: u16,
    #[serde(default = "default_max_fetch_prev_events")]
    pub max_fetch_prev_events: u16,
    #[serde(default = "false_fn")]
    pub allow_registration: bool,
    pub registration_token: Option<String>,
    #[serde(default = "true_fn")]
    pub allow_encryption: bool,
    #[serde(default = "false_fn")]
    pub allow_federation: bool,
    #[serde(default = "false_fn")]
    pub allow_public_room_directory_over_federation: bool,
    #[serde(default = "false_fn")]
    pub allow_public_room_directory_without_auth: bool,
    #[serde(default = "false_fn")]
    pub allow_device_name_federation: bool,
    #[serde(default = "true_fn")]
    pub allow_room_creation: bool,
    #[serde(default = "true_fn")]
    pub allow_unstable_room_versions: bool,
    #[serde(default = "default_default_room_version")]
    pub default_room_version: RoomVersionId,
    pub well_known_client: Option<String>,
    #[serde(default = "false_fn")]
    pub allow_jaeger: bool,
    #[serde(default = "false_fn")]
    pub tracing_flame: bool,
    #[serde(default)]
    pub proxy: ProxyConfig,
    pub jwt_secret: Option<String>,
    #[serde(default = "default_trusted_servers")]
    pub trusted_servers: Vec<OwnedServerName>,
    #[serde(default = "default_log")]
    pub log: String,
    #[serde(default)]
    pub turn_username: String,
    #[serde(default)]
    pub turn_password: String,
    #[serde(default = "Vec::new")]
    pub turn_uris: Vec<String>,
    #[serde(default)]
    pub turn_secret: String,
    #[serde(default = "default_turn_ttl")]
    pub turn_ttl: u64,

    pub emergency_password: Option<String>,

    #[serde(default = "false_fn")]
    pub allow_local_presence: bool,
    #[serde(default = "false_fn")]
    pub allow_incoming_presence: bool,
    #[serde(default = "false_fn")]
    pub allow_outgoing_presence: bool,
    #[serde(default = "default_presence_idle_timeout_s")]
    pub presence_idle_timeout_s: u64,
    #[serde(default = "default_presence_offline_timeout_s")]
    pub presence_offline_timeout_s: u64,

    #[serde(flatten)]
    pub catchall: BTreeMap<String, IgnoredAny>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TlsConfig {
    pub certs: String,
    pub key: String,
}

const DEPRECATED_KEYS: &[&str] = &["cache_capacity"];

impl Config {
    pub fn warn_deprecated(&self) {
        let mut was_deprecated = false;
        for key in self
            .catchall
            .keys()
            .filter(|key| DEPRECATED_KEYS.iter().any(|s| s == key))
        {
            warn!("Config parameter \"{}\" is deprecated.", key);
            was_deprecated = true;
        }

        if was_deprecated {
            warn!("Read conduit documentation and check your configuration if any new configuration parameters should be adjusted");
        }
    }

    /// Checks the presence of the `address` and `unix_socket_path` keys in the raw_config, exiting the process if both keys were detected.
    pub fn error_dual_listening(&self, raw_config: Figment) -> Result<(), ()> {
        let check_address = raw_config.find_value("address");
        let check_unix_socket = raw_config.find_value("unix_socket_path");

        if check_address.is_ok() && check_unix_socket.is_ok() {
            error!("TOML keys \"address\" and \"unix_socket_path\" were both defined. Please specify only one option.");
            return Err(());
        }

        Ok(())
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Prepare a list of config values to show
        let lines = [
            ("Server name", self.server_name.host()),
            ("Database backend", &self.database_backend),
            ("Database path", &self.database_path),
            (
                "Database cache capacity (MB)",
                &self.db_cache_capacity_mb.to_string(),
            ),
            (
                "Cache capacity modifier",
                &self.conduit_cache_capacity_modifier.to_string(),
            ),
            #[cfg(feature = "rocksdb")]
            (
                "Maximum open files for RocksDB",
                &self.rocksdb_max_open_files.to_string(),
            ),
            ("PDU cache capacity", &self.pdu_cache_capacity.to_string()),
            (
                "Cleanup interval in seconds",
                &self.cleanup_second_interval.to_string(),
            ),
            ("Maximum request size", &self.max_request_size.to_string()),
            (
                "Maximum concurrent requests",
                &self.max_concurrent_requests.to_string(),
            ),
            ("Allow registration", &self.allow_registration.to_string()),
            (
                "Enabled lightning bolt",
                &self.enable_lightning_bolt.to_string(),
            ),
            ("Allow encryption", &self.allow_encryption.to_string()),
            ("Allow federation", &self.allow_federation.to_string()),
            (
                "Allow device name federation",
                &self.allow_device_name_federation.to_string(),
            ),
            ("Allow room creation", &self.allow_room_creation.to_string()),
            (
                "Allow public room directory over federation",
                &self.allow_public_room_directory_over_federation.to_string(),
            ),
            (
                "Allow public room directory without authentication",
                &self.allow_public_room_directory_without_auth.to_string(),
            ),
            (
                "JWT secret",
                match self.jwt_secret {
                    Some(_) => "set",
                    None => "not set",
                },
            ),
            ("Trusted servers", {
                let mut lst = vec![];
                for server in &self.trusted_servers {
                    lst.push(server.host());
                }
                &lst.join(", ")
            }),
            (
                "TURN username",
                if self.turn_username.is_empty() {
                    "not set"
                } else {
                    &self.turn_username
                },
            ),
            ("TURN password", {
                if self.turn_password.is_empty() {
                    "not set"
                } else {
                    "set"
                }
            }),
            ("TURN secret", {
                if self.turn_secret.is_empty() {
                    "not set"
                } else {
                    "set"
                }
            }),
            ("Turn TTL", &self.turn_ttl.to_string()),
            ("Turn URIs", {
                let mut lst = vec![];
                for item in self.turn_uris.iter().cloned().enumerate() {
                    let (_, uri): (usize, String) = item;
                    lst.push(uri);
                }
                &lst.join(", ")
            }),
        ];

        let mut msg: String = "Active config values:\n\n".to_owned();

        for line in lines.into_iter().enumerate() {
            msg += &format!("{}: {}\n", line.1 .0, line.1 .1);
        }

        write!(f, "{msg}")
    }
}

fn false_fn() -> bool {
    false
}

fn true_fn() -> bool {
    true
}

fn default_address() -> IpAddr {
    Ipv4Addr::LOCALHOST.into()
}

fn default_port() -> u16 {
    8000
}

fn default_unix_socket_perms() -> u32 {
    660
}

fn default_database_backend() -> String {
    "sqlite".to_owned()
}

fn default_db_cache_capacity_mb() -> f64 {
    300.0
}

fn default_conduit_cache_capacity_modifier() -> f64 {
    1.0
}

fn default_rocksdb_max_open_files() -> i32 {
    1000
}

fn default_pdu_cache_capacity() -> u32 {
    150_000
}

fn default_cleanup_second_interval() -> u32 {
    60 // every minute
}

fn default_max_request_size() -> u32 {
    20 * 1024 * 1024 // Default to 20 MB
}

fn default_max_concurrent_requests() -> u16 {
    100
}

fn default_max_fetch_prev_events() -> u16 {
    100_u16
}

fn default_trusted_servers() -> Vec<OwnedServerName> {
    vec![OwnedServerName::try_from("matrix.org").unwrap()]
}

fn default_log() -> String {
    "warn,state_res=warn,_=off,sled=off".to_owned()
}

fn default_turn_ttl() -> u64 {
    60 * 60 * 24
}

fn default_presence_idle_timeout_s() -> u64 {
    5 * 60
}

fn default_presence_offline_timeout_s() -> u64 {
    15 * 60
}

// I know, it's a great name
pub fn default_default_room_version() -> RoomVersionId {
    RoomVersionId::V10
}
