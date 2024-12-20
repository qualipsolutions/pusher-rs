# pusher-rs

A robust Rust client library for interacting with the Pusher Channels API. This library provides a simple and efficient way to integrate Pusher's real-time functionality into your Rust applications.

## Features

- [x] WebSocket-based real-time connection with automatic socket ID handling
- [x] Support for all channel types:
  - Public channels
  - Private channels
  - Presence channels
  - Private encrypted channels
- [x] Event publishing and subscription
- [x] Batch event triggering
- [x] Automatic reconnection with exponential backoff
- [x] Connection state management
- [x] Environment-based configuration
- [x] Comprehensive error handling
- [x] TLS support
- [x] Presence channel user authentication
- [x] Channel encryption/decryption

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
pusher-rs = "0.1.2"
```

## Configuration

The library uses environment variables for configuration. Create a `.env` file in your project root:

```env
PUSHER_APP_ID=your_app_id
PUSHER_KEY=your_app_key
PUSHER_SECRET=your_app_secret
PUSHER_CLUSTER=your_cluster
PUSHER_USE_TLS=true
```

## Usage Examples

### Basic Connection and Events

```rust
use pusher_rs::{PusherClient, PusherConfig};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize from environment variables
    let config = PusherConfig::from_env()?;
    let mut client = PusherClient::new(config)?;

    // Connect and wait for socket ID
    client.connect().await?;

    // Subscribe to a public channel
    client.subscribe("my-channel").await?;

    // Bind to events
    client.bind("my-event", |event| {
        println!("Received event: {:?}", event);
    }).await?;

    // Trigger an event
    client.trigger(
        "my-channel",
        "my-event",
        &json!({"message": "Hello, World!"}).to_string()
    ).await?;

    Ok(())
}
```

### Presence Channel Example

```rust
use pusher_rs::{PusherClient, PusherConfig};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = PusherConfig::from_env()?;
    let mut client = PusherClient::new(config)?;

    // Connect and wait for socket ID
    client.connect().await?;
    let socket_id = client.get_socket_id().await?.unwrap();

    // Set up presence channel data
    let channel = "presence-my-channel";
    let user_id = "user_123";
    let user_info = json!({
        "name": "John Doe",
        "email": "john@example.com"
    });

    // Create channel data
    let channel_data = json!({
        "user_id": user_id,
        "user_info": user_info
    });
    let channel_data_str = serde_json::to_string(&channel_data)?;

    // Get auth signature and subscribe
    let auth = client.authenticate_presence_channel(
        &socket_id,
        channel,
        user_id,
        Some(&user_info)
    )?;
    client.subscribe_with_auth(channel, &auth, Some(&channel_data_str)).await?;

    Ok(())
}
```

### Private Encrypted Channel Example

```rust
use pusher_rs::{PusherClient, PusherConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = PusherClient::new(PusherConfig::from_env()?)?;
    client.connect().await?;

    // Subscribe to encrypted channel
    client.subscribe_encrypted("private-encrypted-channel").await?;

    // Trigger encrypted event
    client.trigger_encrypted(
        "private-encrypted-channel",
        "my-event",
        &json!({"secret": "message"}).to_string()
    ).await?;

    Ok(())
}
```

### Batch Events Example

```rust
use pusher_rs::{PusherClient, PusherConfig, BatchEvent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = PusherClient::new(PusherConfig::from_env()?)?;

    let batch_events = vec![
        BatchEvent {
            channel: "channel-1".to_string(),
            event: "event-1".to_string(),
            data: json!({"message": "Hello from event 1"}).to_string(),
        },
        BatchEvent {
            channel: "channel-2".to_string(),
            event: "event-2".to_string(),
            data: json!({"message": "Hello from event 2"}).to_string(),
        },
    ];

    client.trigger_batch(batch_events).await?;
    Ok(())
}
```

### Connection State Handling

```rust
use pusher_rs::{PusherClient, PusherConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = PusherClient::new(PusherConfig::from_env()?)?;

    // Bind to connection events
    client.on_connect(|| {
        println!("Connected to Pusher!");
    }).await?;

    client.on_disconnect(|| {
        println!("Disconnected from Pusher!");
    }).await?;

    // Connect and check state
    client.connect().await?;
    assert!(client.is_connected().await);

    // Get current state
    let state = client.get_connection_state().await;
    println!("Current state: {:?}", state);

    Ok(())
}
```

## Error Handling

The library provides comprehensive error handling through the `PusherError` type:

```rust
use pusher_rs::{PusherClient, PusherConfig, PusherError};

#[tokio::main]
async fn main() {
    match PusherClient::new(PusherConfig::from_env()) {
        Ok(mut client) => {
            match client.connect().await {
                Ok(_) => println!("Connected successfully"),
                Err(PusherError::ConnectionError(e)) => println!("Connection error: {}", e),
                Err(PusherError::AuthError(e)) => println!("Authentication error: {}", e),
                Err(e) => println!("Other error: {}", e),
            }
        }
        Err(e) => println!("Failed to create client: {}", e),
    }
}
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License.
