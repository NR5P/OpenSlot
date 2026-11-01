//! Integration test against a live VARA-compatible modem (xara).
//! Run with: cargo test --test live_modem -- --nocapture --ignored
//! Requires xara running on localhost:8300/8301.

use std::time::Duration;
use tokio::time::sleep;
use openslot::vara::commands::{Bandwidth, ListenMode, VaraCommand};
use openslot::vara::connection::VaraConnection;
use openslot::varac::messages::ServiceFlags;
use openslot::varac::session::{StationConfig, VaracSession};

#[tokio::test]
#[ignore]
async fn test_modem_connect_and_commands() {
    println!("\n=== Live Modem Integration Test ===\n");

    // 1. Connect to modem
    println!("[1] Connecting to VARA modem on 127.0.0.1:8300/8301...");
    let mut vara = VaraConnection::connect("127.0.0.1", 8300, 8301)
        .await
        .expect("Failed to connect to modem");
    println!("    OK - connected");

    // Drain initial events (BUSY, REGISTERED, etc.)
    sleep(Duration::from_millis(500)).await;
    drain_events(&mut vara, "init").await;

    // 2. Set callsign
    println!("[2] MYCALL NR5P...");
    vara.send_command(&VaraCommand::MyCall(vec!["NR5P".to_string()]))
        .await
        .expect("MYCALL failed");
    sleep(Duration::from_millis(300)).await;
    drain_events(&mut vara, "MYCALL").await;

    // 3. Set bandwidth
    println!("[3] BW2300...");
    vara.send_command(&VaraCommand::SetBandwidth(Bandwidth::Bw2300))
        .await
        .expect("BW2300 failed");
    sleep(Duration::from_millis(300)).await;
    drain_events(&mut vara, "BW2300").await;

    // 4. Listen ON
    println!("[4] LISTEN ON...");
    vara.send_command(&VaraCommand::Listen(ListenMode::On))
        .await
        .expect("LISTEN ON failed");
    sleep(Duration::from_millis(300)).await;
    drain_events(&mut vara, "LISTEN ON").await;

    // 5. Create session
    println!("[5] Creating VaracSession...");
    let config = StationConfig {
        callsign: "NR5P".to_string(),
        name: Some("Test".to_string()),
        qth: Some("Test QTH".to_string()),
        locator: Some("EM00".to_string()),
        services: ServiceFlags(ServiceFlags::VMAIL),
        bbs_dir: None,
        email_gate: None,
        bbs_gate: None,
        policy: Default::default(),
    };
    let mut session = VaracSession::new(vara, config);
    println!("    OK - session created, state: {:?}", session.state);

    // 6. Send beacon
    println!("[6] Sending beacon (CQFRAME NR5P-9 2300)...");
    session
        .vara
        .send_command(&VaraCommand::CqFrame {
            source: "NR5P-9".to_string(),
            bandwidth: 2300,
        })
        .await
        .expect("CQ beacon failed");
    sleep(Duration::from_millis(2000)).await;
    drain_session_events(&mut session, "beacon").await;

    // 7. Send CQ
    println!("[7] Sending CQ (CQFRAME NR5P-1 2300)...");
    session
        .vara
        .send_command(&VaraCommand::CqFrame {
            source: "NR5P-1".to_string(),
            bandwidth: 2300,
        })
        .await
        .expect("CQ failed");
    sleep(Duration::from_millis(2000)).await;
    drain_session_events(&mut session, "CQ").await;

    // 8. Bandwidth cycling
    println!("[8] Cycling bandwidths...");
    for bw in [Bandwidth::Bw500, Bandwidth::Bw2750, Bandwidth::Bw2300] {
        session
            .vara
            .send_command(&VaraCommand::SetBandwidth(bw))
            .await
            .expect("BW change failed");
        sleep(Duration::from_millis(300)).await;
        println!("    Set to {:?} - OK", bw);
    }
    drain_session_events(&mut session, "BW cycle").await;

    // 9. Listen mode cycling
    println!("[9] Cycling listen modes...");
    for mode in [ListenMode::Cq, ListenMode::Off, ListenMode::On] {
        session
            .vara
            .send_command(&VaraCommand::Listen(mode))
            .await
            .expect("Listen mode change failed");
        sleep(Duration::from_millis(300)).await;
        println!("    Set to {:?} - OK", mode);
    }
    drain_session_events(&mut session, "listen cycle").await;

    // 10. Connect attempt (will fail - no remote station)
    println!("[10] Attempting CONNECT to W1TEST (expect failure)...");
    session
        .vara
        .send_command(&VaraCommand::Connect {
            source: "NR5P".to_string(),
            destination: "W1TEST".to_string(),
        })
        .await
        .expect("CONNECT command failed");
    sleep(Duration::from_millis(3000)).await;
    drain_session_events(&mut session, "CONNECT").await;

    // 11. Abort
    println!("[11] Sending ABORT...");
    session
        .vara
        .send_command(&VaraCommand::Abort)
        .await
        .expect("ABORT failed");
    sleep(Duration::from_millis(1000)).await;
    drain_session_events(&mut session, "ABORT").await;

    println!("\n=== All tests passed! ===");
}

async fn drain_events(vara: &mut VaraConnection, label: &str) {
    loop {
        match tokio::time::timeout(Duration::from_millis(200), vara.event_rx.recv()).await {
            Ok(Some(evt)) => println!("    [{label}] event: {evt:?}"),
            _ => break,
        }
    }
}

async fn drain_session_events(session: &mut VaracSession, label: &str) {
    loop {
        match tokio::time::timeout(Duration::from_millis(200), session.poll()).await {
            Ok(Some(evt)) => println!("    [{label}] event: {evt:?}"),
            _ => break,
        }
    }
}
