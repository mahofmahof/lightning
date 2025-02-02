use std::env;

#[derive(Clone)]
pub struct Config {
    pub db_path: String,
    pub ipinfo_token: String,
    pub tracker_port: u16,
    pub lgtn_node_address: String,
    pub lgtn_node_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: env::var("DB_PATH")
                .unwrap_or("~/.lightning/data/geo_info_db".to_string())
                .parse()
                .unwrap(),
            ipinfo_token: env::var("IPINFO_TOKEN").expect("ip info api token should be set"),
            tracker_port: env::var("TRACKER_PORT")
                .unwrap_or("4000".to_string())
                .parse()
                .unwrap_or(4000),
            lgtn_node_address: env::var("LGTN_ADDRESS").expect("LGTN_ADDRESS should be set"),
            lgtn_node_port: env::var("LGTN_PORT")
                .expect("LGTN_PORT should be set")
                .parse()
                .expect("LGTN_PORT should be an integer"),
        }
    }
}
