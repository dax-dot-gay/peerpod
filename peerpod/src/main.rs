use std::time::Duration;

use peerpod::{node::NodeBuilder, types::Error};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut node = NodeBuilder::default().add_rendezvous(
        "12D3KooWRW5KgEc71mvCcd2hFtuH3HhthJRSpeM7h4LURDif9cMF".to_string(),
        "/ip4/192.168.0.41/tcp/8080".to_string(),
    )?
    .add_relay(
        "12D3KooWRW5KgEc71mvCcd2hFtuH3HhthJRSpeM7h4LURDif9cMF".to_string(),
        "/ip4/192.168.0.41/tcp/8080".to_string(),
    )?.class("test".to_string()).listen_address("/ip4/0.0.0.0/tcp/0".to_string()).build().expect("Failed to build");
    let _ = node.initialize();
    node.on_request::<String, String, Error>("/test".to_string(), |val| {
        println!("{val}");
        Ok(String::new())
    }).await?;
    node.on_event(peerpod::types::EventType::Discovered, |ev| println!("{ev:?}")).await?;
    node.on_event(peerpod::types::EventType::RequestFailed, |ev| println!("{ev:?}")).await?;
    loop {
        println!("LOOP");
        for peer in node.get_peers().await? {
            println!("PEER: {peer:?}");
            if peer.id.to_string() != "12D3KooWRW5KgEc71mvCcd2hFtuH3HhthJRSpeM7h4LURDif9cMF".to_string() {
                println!("SENDING TO: {:?}", peer);
                println!("{:?}", node.request::<String, String, Error>(peer.id.to_string(), "/test".to_string(), "BEANS!".to_string()).await);
            }
            
        }
        sleep(Duration::from_secs(2)).await;
    }
}
