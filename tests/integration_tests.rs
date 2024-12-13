use env_logger::Env;
use once_cell::sync::OnceCell;
use pusher_rs::{ConnectionState, Event, PusherClient, PusherConfig};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio::time::{sleep, Duration};

static INIT: OnceCell<()> = OnceCell::new();

fn init_logger() {
    INIT.get_or_init(|| {
        let env = Env::default()
            .filter_or("MY_LOG_LEVEL", "debug")
            .write_style_or("MY_LOG_STYLE", "always");
        env_logger::init_from_env(env);
    });
}

async fn setup_client() -> PusherClient {
    init_logger();
    let config =
        PusherConfig::from_env().expect("Failed to load Pusher configuration from environment");
    PusherClient::new(config).unwrap()
}

#[tokio::test]
async fn test_pusher_client_connection() {
    let mut client = setup_client().await;

    client.connect().await.unwrap();
    assert_eq!(
        client.get_connection_state().await,
        ConnectionState::Connected
    );

    // Verify socket ID is not empty
    let socket_id = client.get_socket_id().await.unwrap();
    assert!(socket_id.is_some(), "Socket ID should be set after connection");
    assert!(!socket_id.unwrap().is_empty(), "Socket ID should not be empty");

    client.disconnect().await.unwrap();
    assert_eq!(
        client.get_connection_state().await,
        ConnectionState::Disconnected
    );
}

#[tokio::test]
async fn test_channel_subscription() {
    let mut client = setup_client().await;

    // Connect with a timeout
    match timeout(Duration::from_secs(10), client.connect()).await {
        Ok(result) => {
            result.expect("Failed to connect to Pusher");
        }
        Err(_) => panic!("Connection timed out"),
    }

    // Ensure we're connected
    assert_eq!(
        client.get_connection_state().await,
        ConnectionState::Connected
    );

    // Verify socket ID is set before subscribing
    let socket_id = client.get_socket_id().await.unwrap();
    assert!(socket_id.is_some(), "Socket ID should be set before subscribing");
    assert!(!socket_id.unwrap().is_empty(), "Socket ID should not be empty");

    // Subscribe to the channel
    match timeout(Duration::from_secs(5), client.subscribe("test-channel")).await {
        Ok(result) => {
            result.expect("Failed to subscribe to channel");
        }
        Err(_) => panic!("Subscription timed out"),
    }

    // Wait a bit for the subscription to be processed
    tokio::time::sleep(Duration::from_secs(1)).await;

    let channels = client.get_subscribed_channels().await;
    log::info!("Subscribed channels: {:?}", channels);
    assert!(channels.contains(&"test-channel".to_string()), "Channel not found in subscribed channels");
}

#[tokio::test]
async fn test_event_binding() {
    let mut client = setup_client().await;

    // Connect to Pusher
    client.connect().await.unwrap();
    assert_eq!(
        client.get_connection_state().await,
        ConnectionState::Connected
    );

    // Verify socket ID is set
    let socket_id = client.get_socket_id().await.unwrap();
    assert!(socket_id.is_some(), "Socket ID should be set after connection");
    assert!(!socket_id.unwrap().is_empty(), "Socket ID should not be empty");

    // Create a flag to track if the event was received
    let event_received = Arc::new(RwLock::new(false));
    let event_received_clone = event_received.clone();

    // Subscribe to the test channel first
    client.subscribe("test-channel").await.unwrap();

    // Bind to the test event
    client
        .bind("test-event", move |_| {
            let event_received = event_received_clone.clone();
            tokio::spawn(async move {
                let mut flag = event_received.write().await;
                *flag = true;
            });
        })
        .await
        .unwrap();

    // Wait a bit for the binding to be processed
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Trigger the event
    client.trigger("test-channel", "test-event", "{}").await.unwrap();

    // Wait for the event to be processed
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify the event was received
    assert!(*event_received.read().await, "Event should have been received");
}

#[tokio::test]
#[ignore]
async fn test_encrypted_channel() {
    let mut client = setup_client().await;

    client.connect().await.unwrap();
    client
        .subscribe_encrypted("private-encrypted-channel")
        .await
        .unwrap();

    let channels = client.get_subscribed_channels().await;
    assert!(channels.contains(&"private-encrypted-channel".to_string()));

    // TODO - Test sending and receiving encrypted messages
}

#[tokio::test]
async fn test_send_payload() {
    let mut client = setup_client().await;

    // Connect with a timeout
    match timeout(Duration::from_secs(10), client.connect()).await {
        Ok(result) => {
            result.expect("Failed to connect to Pusher");
        }
        Err(_) => panic!("Connection timed out"),
    }

    // Ensure we're connected
    assert_eq!(
        client.get_connection_state().await,
        ConnectionState::Connected
    );

    let test_channel = "test-channel-payload";
    let test_event = "test-event-payload";
    let test_data = r#"{"message": "Hello, Pusher!"}"#;

    // Subscribe to the channel
    client
        .subscribe(test_channel)
        .await
        .expect("Failed to subscribe to channel");

    // Set up event binding to capture the triggered event
    let event_received = Arc::new(RwLock::new(false));
    let event_received_clone = event_received.clone();
    let received_data = Arc::new(RwLock::new(None));
    let received_data_clone = received_data.clone();

    client
        .bind(test_event, move |event: Event| {
            let event_received = event_received_clone.clone();
            let received_data = received_data_clone.clone();
            tokio::spawn(async move {
                let mut flag = event_received.write().await;
                *flag = true;
                let mut data = received_data.write().await;
                *data = Some(event.data);
            });
        })
        .await
        .expect("Failed to bind event");

    // Create and trigger the event
    client.trigger(test_channel, test_event, test_data).await.unwrap();
    println!("Event triggered successfully");

    // Wait for the event to be processed
    sleep(Duration::from_secs(1)).await;

    // Assert that the event was received and processed
    assert!(*event_received.read().await, "Event was not received");

    // Assert that the received data matches the sent data
    let received = received_data.read().await;
    assert_eq!(
        received.as_ref().unwrap().trim(),
        test_data.trim(),
        "Received data does not match sent data"
    );

    client
        .unsubscribe(test_channel)
        .await
        .expect("Failed to unsubscribe from channel");
    client.disconnect().await.expect("Failed to disconnect");
}