use libp2p::futures::StreamExt;
use peerpod::{node::NodeBuilder, types::Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut node = NodeBuilder::default().add_rendezvous(
        "12D3KooWRW5KgEc71mvCcd2hFtuH3HhthJRSpeM7h4LURDif9cMF".to_string(),
        "/ip4/192.168.0.41/tcp/8080".to_string(),
    )?
    .add_relay(
        "12D3KooWRW5KgEc71mvCcd2hFtuH3HhthJRSpeM7h4LURDif9cMF".to_string(),
        "/ip4/192.168.0.41/tcp/8080".to_string(),
    )?.class("test".to_string()).listen_address("/ip4/0.0.0.0/tcp/0".to_string()).friendly_name(None).build().expect("Failed to build");
    let _ = node.initialize();
    loop {
        if let Some(ref recv) = node.event_receiver {
            let mut pinned = Box::pin(recv.clone());
            if let Some(event) = pinned.next().await {
                println!("{event:?}");
            } else {
                break;
            }
        } else {
            break;
        }
    }
    Ok(())
}
