use std::{net::SocketAddr, time::Duration};

use clap::{Parser, Subcommand};
use fleek_crypto::NodeNetworkingPublicKey;
use lightning_application::query_runner::QueryRunner;
use lightning_dht::dht::{Builder, Dht};
use lightning_interfaces::{Blake3Hash, TopologyInterface};
use lightning_topology::Topology;

const BOOTSTRAP_KEY: Blake3Hash = [
    240, 76, 40, 117, 207, 118, 89, 141, 116, 76, 54, 143, 23, 169, 217, 135, 248, 10, 42, 172, 64,
    171, 193, 85, 186, 234, 102, 129, 48, 240, 126, 33,
];

#[derive(Parser)]
struct Cli {
    #[arg(short, long, group = "bootstrap_address")]
    bootstrapper: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[group(required = true)]
enum Commands {
    Get {
        #[arg(short, long)]
        key: String,
    },
    Put,
    Join,
    Bootstrapper,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Get { key } => {
            let address: SocketAddr = cli.bootstrapper.unwrap().parse().unwrap();
            let public_key = NodeNetworkingPublicKey(rand::random());
            tracing::info!("public key: {public_key:?}");
            let dht =
                start_node::<Topology<QueryRunner>>(public_key, Some((address, BOOTSTRAP_KEY)))
                    .await;

            tracing::info!("GET {key:?}");

            let key = hex::decode(key).unwrap();
            if let Some(value) = dht.get(&key).await {
                tracing::info!("value found is {:?}", value.value);
            }
        },
        Commands::Put => {
            let address: SocketAddr = cli.bootstrapper.unwrap().parse().unwrap();
            let public_key = NodeNetworkingPublicKey(rand::random());
            tracing::info!("public key: {public_key:?}");
            let dht =
                start_node::<Topology<QueryRunner>>(public_key, Some((address, BOOTSTRAP_KEY)))
                    .await;

            // Todo: get actual hash.
            let key: Blake3Hash = rand::random();
            let value: [u8; 4] = rand::random();

            tracing::info!("PUT {}:{value:?}", hex::encode(key));

            dht.put(&key, &value);

            // Todo: Let's remove this loop.
            // We have this loop so that the spawn task of `put` finishes.
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        },
        Commands::Join => {
            let address: SocketAddr = cli.bootstrapper.unwrap().parse().unwrap();
            let public_key = NodeNetworkingPublicKey(rand::random());
            tracing::info!("public key: {public_key:?}");
            let _ = start_node::<Topology<QueryRunner>>(public_key, Some((address, BOOTSTRAP_KEY)))
                .await;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        },
        Commands::Bootstrapper => {
            let _ =
                start_node::<Topology<QueryRunner>>(NodeNetworkingPublicKey(BOOTSTRAP_KEY), None)
                    .await;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        },
    }

    tracing::info!("shutting down dht-node");
}

async fn start_node<T: TopologyInterface>(
    public_key: NodeNetworkingPublicKey,
    bootstrapper: Option<(SocketAddr, Blake3Hash)>,
) -> Dht<T> {
    let mut builder = Builder::new();
    builder.set_node_key(public_key);

    if let Some((address, key)) = bootstrapper {
        tracing::info!("bootstrapping to {address:?} {key:?}");
        builder.add_node(NodeNetworkingPublicKey(key), address);
    }

    let dht = builder.build().await.unwrap();

    tracing::info!("start bootstrap");
    dht.bootstrap().await;

    while !dht.is_bootstrapped().await {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    tracing::info!("finished bootstrapping");

    dht
}
