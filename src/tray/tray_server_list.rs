// Process the data acquired from net_status for server list in Tray Menu

use std::collections::HashMap;
use isocountry::CountryCode;

use geph5_broker_protocol::{
    ExitCategory,
    NetStatus,
    AccountLevel,
};


/// Struct for Tray to store cached exit/server info 
#[derive(Clone, Debug)]
pub(super) struct TrayServerEntry {
    /// Display country code, e.g. "CA"
    pub country: String,

    /// Display city name, e.g. "Montreal"
    pub city: String,

    pub country_code: CountryCode,

    /// Actual exit hostname used by ExitConstraint::Hostname()
    pub hostname: String,

    /// Core / Streaming
    pub category: ExitCategory,

    /// Selected backend exit load
    pub load: f32,

    /// Whether Free account can use this server
    pub allowed_levels: Vec<AccountLevel>,
}


/// Aggregated server list for Tray menu.
///
/// Future menu layout:
///
/// Auto
/// -----
/// Core
///   CA Montreal
///   US San Jose
/// -----
/// Streaming
///   TW Taipei
///
#[derive(Clone, Debug, Default)]
pub(super) struct TrayServerList {
    pub core: Vec<TrayServerEntry>,
    pub streaming: Vec<TrayServerEntry>,
}


impl TrayServerList {

    /// Convert raw NetStatus into Tray-friendly server list.
    ///
    /// NetStatus example:
    ///
    /// CA Montreal hostname=A load=0.8
    /// CA Montreal hostname=B load=0.6
    ///
    /// Result:
    ///
    /// CA Montreal hostname=B load=0.6
    ///
    pub fn from_net_status(status: NetStatus) -> Self {

        /*
         * Temporary map:
         *
         * key:
         *   (country, city, category)
         *
         * value:
         *   best load exit
         *
         * Used to merge duplicated exits.
         */
        let mut best_servers:
            HashMap<(String, String, String), TrayServerEntry>
            = HashMap::new();


        for (hostname, (_pk, exit, meta)) in status.exits {

            let server = TrayServerEntry {
                country: exit.country.alpha2().to_string(),
                city: exit.city.clone(),
                country_code: exit.country.clone(),
                hostname,
                category: meta.category.clone(),
                load: exit.load,
                allowed_levels: meta.allowed_levels.clone(),
            };


            let key = (
                server.country.clone(),
                server.city.clone(),
                format!("{:?}", server.category),
            );


            match best_servers.get(&key) {

                /*
                 * Existing server:
                 *
                 * Keep lower load exit.
                 */
                Some(old) => {
                    if server.load < old.load {
                        best_servers.insert(key, server);
                    }
                }


                /*
                 * First exit for this city/category.
                 */
                None => {
                    best_servers.insert(key, server);
                }
            }
        }


        let mut result = TrayServerList::default();


        for (_, server) in best_servers {

            match server.category {

                ExitCategory::Core => {
                    result.core.push(server);
                }

                ExitCategory::Streaming => {
                    result.streaming.push(server);
                }

                /*
                 * Future category extension:
                 * currently ignore unknown category.
                 */
                // _ => {}
            }
        }


        // Keep menu order deterministic.
        result.core.sort_by(|a, b| {
            (
                a.country.as_str(),
                a.city.as_str()
            )
            .cmp(&(
                b.country.as_str(),
                b.city.as_str()
            ))
        });


        result.streaming.sort_by(|a, b| {
            (
                a.country.as_str(),
                a.city.as_str()
            )
            .cmp(&(
                b.country.as_str(),
                b.city.as_str()
            ))
        });


        result
    }

    // A simple check for server availability
    // currently geph has only core and streaming server types
    // if more server types added in future, needing modification or restructure
    pub fn is_empty(&self) -> bool {
        self.core.is_empty()
            && self.streaming.is_empty()
    }
}