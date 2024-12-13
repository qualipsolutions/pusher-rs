/// A client for interacting with the Pusher service.
///
mod auth;
mod channels;
mod config;
mod error;
mod events;
mod websocket;

use aes::{
    cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit},
    Aes256,
};
use cbc::Encryptor;
use hmac::{Hmac, Mac};
use log::info;
use rand::Rng;
use serde_json::json;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use url::Url;
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub use auth::PusherAuth;
pub use channels::{Channel, ChannelType};
pub use config::PusherConfig;
pub use error::{PusherError, PusherResult};
pub use events::{Event, SystemEvent};

use websocket::{WebSocketClient, WebSocketCommand};

/// This struct provides methods for connecting to Pusher, subscribing to channels,
/// triggering events, and handling incoming events.
pub struct PusherClient {
    config: PusherConfig,
    auth: PusherAuth,
    // websocket: Option<WebSocketClient>,
    websocket_command_tx: Option<mpsc::Sender<WebSocketCommand>>,
    channels: Arc<RwLock<HashMap<String, Channel>>>,
    event_handlers: Arc<RwLock<HashMap<String, Vec<Box<dyn Fn(Event) + Send + Sync + 'static>>>>>,
    state: Arc<RwLock<ConnectionState>>,
    event_tx: mpsc::Sender<Event>,
    encrypted_channels: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    socket_id: Arc<RwLock<Option<String>>>,
}

#[derive(Debug, Clone)]
pub struct BatchEvent {
    pub channel: String,
    pub event: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

impl PusherClient {
    /// Creates a new `PusherClient` instance with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration for the Pusher client.
    ///
    /// # Returns
    ///
    /// A `PusherResult` containing the new `PusherClient` instance.
    pub fn new(config: PusherConfig) -> PusherResult<Self> {
        let auth = PusherAuth::new(&config.app_key, &config.app_secret);
        let (event_tx, event_rx) = mpsc::channel(100);
        let state = Arc::new(RwLock::new(ConnectionState::Disconnected));
        let event_handlers = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let encrypted_channels = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let socket_id = Arc::new(RwLock::new(None));

        let client = Self {
            config,
            auth,
            websocket_command_tx: None,
            channels: Arc::new(RwLock::new(std::collections::HashMap::new())),
            event_handlers: event_handlers.clone(),
            state: state.clone(),
            event_tx,
            encrypted_channels,
            socket_id,
        };

        tokio::spawn(Self::handle_events(event_rx, event_handlers));

        Ok(client)
    }

    async fn send(&self, message: String) -> PusherResult<()> {
        if let Some(tx) = &self.websocket_command_tx {
            tx.send(WebSocketCommand::Send(message))
                .await
                .map_err(|e| {
                    PusherError::WebSocketError(format!("Failed to send command: {}", e))
                })?;
            Ok(())
        } else {
            Err(PusherError::ConnectionError("Not connected".into()))
        }
    }

    async fn handle_events(
        mut event_rx: mpsc::Receiver<Event>,
        event_handlers: Arc<
            RwLock<
                std::collections::HashMap<String, Vec<Box<dyn Fn(Event) + Send + Sync + 'static>>>,
            >,
        >,
    ) {
        while let Some(event) = event_rx.recv().await {
            let handlers = event_handlers.read().await;
            if let Some(callbacks) = handlers.get(&event.event) {
                for callback in callbacks {
                    callback(event.clone());
                }
            }
        }
    }

    /// Connects to the Pusher server.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn connect(&mut self) -> PusherResult<()> {
        let url = self.get_websocket_url()?;
        let (command_tx, command_rx) = mpsc::channel(100);

        let mut websocket = WebSocketClient::new(
            url.clone(),
            Arc::clone(&self.state),
            self.event_tx.clone(),
            command_rx,
            Arc::clone(&self.socket_id),
        );

        log::info!("Connecting to Pusher using URL: {}", url);
        websocket.connect().await?;

        tokio::spawn(async move {
            websocket.run().await;
        });

        self.websocket_command_tx = Some(command_tx);

        Ok(())
    }

    /// Disconnects from the Pusher server.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn disconnect(&mut self) -> PusherResult<()> {
        if let Some(tx) = self.websocket_command_tx.take() {
            tx.send(WebSocketCommand::Close).await.map_err(|e| {
                PusherError::WebSocketError(format!("Failed to send close command: {}", e))
            })?;
        }
        *self.state.write().await = ConnectionState::Disconnected;
        Ok(())
    }

    /// Subscribes to a channel.
    ///
    /// # Arguments
    ///
    /// * `channel_name` - The name of the channel to subscribe to.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn subscribe(&mut self, channel_name: &str) -> PusherResult<()> {
        let channel = Channel::new(channel_name);
        let mut channels = self.channels.write().await;
        channels.insert(channel_name.to_string(), channel);

        let data = json!({
            "event": "pusher:subscribe",
            "data": {
                "channel": channel_name
            }
        });

        self.send(serde_json::to_string(&data)?).await
    }


    /// Subscribes to an encrypted channel.
    ///
    /// # Arguments
    ///
    /// * `channel_name` - The name of the encrypted channel to subscribe to.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn subscribe_encrypted(&mut self, channel_name: &str) -> PusherResult<()> {
        if !channel_name.starts_with("private-encrypted-") {
            return Err(PusherError::ChannelError(
                "Encrypted channels must start with 'private-encrypted-'".to_string(),
            ));
        }

        let shared_secret = self.generate_shared_secret(channel_name);

        {
            let mut encrypted_channels = self.encrypted_channels.write().await;
            encrypted_channels.insert(channel_name.to_string(), shared_secret);
        }

        self.subscribe(channel_name).await
    }

    /// Unsubscribes from a channel.
    ///
    /// # Arguments
    ///
    /// * `channel_name` - The name of the channel to unsubscribe from.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    ///
    pub async fn unsubscribe(&mut self, channel_name: &str) -> PusherResult<()> {
        {
            let mut channels = self.channels.write().await;
            channels.remove(channel_name);
        }

        {
            let mut encrypted_channels = self.encrypted_channels.write().await;
            encrypted_channels.remove(channel_name);
        }

        let data = json!({
            "event": "pusher:unsubscribe",
            "data": {
                "channel": channel_name
            }
        });

        self.send(serde_json::to_string(&data)?).await
    }

    /// Triggers an event on a channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The name of the channel to trigger the event on.
    /// * `event` - The name of the event to trigger.
    /// * `data` - The data to send with the event.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn trigger(&self, channel: &str, event: &str, data: &str) -> PusherResult<()> {
        let url = format!(
            "https://api-{}.pusher.com/apps/{}/events",
            self.config.cluster, self.config.app_id
        );

        // Validate that the data is valid JSON, but keep it as a string
        serde_json::from_str::<serde_json::Value>(data)
            .map_err(|e| PusherError::JsonError(e))?;

        let body = json!({
            "name": event,
            "channel": channel,
            "data": data, // Keep data as a string
        });
        let path = format!("/apps/{}/events", self.config.app_id);
        let auth_params = self.auth.authenticate_request("POST", &path, &body)?;

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&body)
            .query(&auth_params)
            .send()
            .await?;
        let response_status = response.status();
        if response_status.is_success() {
            Ok(())
        } else {
            let error_body = response.text().await?;
            Err(PusherError::ApiError(format!(
                "Failed to trigger event: {} - {}",
                response_status, error_body
            )))
        }
    }

    /// Triggers an event on an encrypted channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The name of the encrypted channel to trigger the event on.
    /// * `event` - The name of the event to trigger.
    /// * `data` - The data to send with the event.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn trigger_encrypted(
        &self,
        channel: &str,
        event: &str,
        data: &str,
    ) -> PusherResult<()> {
        let shared_secret = {
            let encrypted_channels = self.encrypted_channels.read().await;
            encrypted_channels
                .get(channel)
                .ok_or_else(|| {
                    PusherError::ChannelError(
                        "Channel is not subscribed or is not encrypted".to_string(),
                    )
                })?
                .clone()
        };

        let encrypted_data = self.encrypt_data(data, &shared_secret)?;
        self.trigger(channel, event, &encrypted_data).await
    }

    /// Triggers multiple events in a single API call.
    ///
    /// # Arguments
    ///
    /// * `batch_events` - A vector of `BatchEvent` structs, each containing channel, event, and data.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn trigger_batch(&self, batch_events: Vec<BatchEvent>) -> PusherResult<()> {
        let url = format!(
            "https://api-{}.pusher.com/apps/{}/batch_events",
            self.config.cluster, self.config.app_id
        );

        let events: Vec<serde_json::Value> = batch_events
            .into_iter()
            .map(|event| {
                json!({
                    "channel": event.channel,
                    "name": event.event,
                    "data": event.data
                })
            })
            .collect();

        let body = json!({ "batch": events });
        let path = format!("/apps/{}/batch_events", self.config.app_id);
        let auth_params = self.auth.authenticate_request("POST", &path, &body)?;

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&body)
            .query(&auth_params)
            .send()
            .await?;

        let response_status = response.status();
        if response_status.is_success() {
            Ok(())
        } else {
            let error_body = response.text().await?;
            Err(PusherError::ApiError(format!(
                "Failed to trigger batch events: {} - {}",
                response_status, error_body
            )))
        }
    }

    /// Binds a callback to an event.
    ///
    /// # Arguments
    ///
    /// * `event_name` - The name of the event to bind to.
    /// * `callback` - The callback function to execute when the event occurs.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    ///
    pub async fn bind<F>(&self, event_name: &str, callback: F) -> PusherResult<()>
    where
        F: Fn(Event) + Send + Sync + 'static,
    {
        let mut handlers = self.event_handlers.write().await;
        handlers
            .entry(event_name.to_string())
            .or_insert_with(Vec::new)
            .push(Box::new(callback));
        Ok(())
    }

    fn get_websocket_url(&self) -> PusherResult<Url> {
        let scheme = if self.config.use_tls { "wss" } else { "ws" };
        info!("Connecting to Pusher using scheme: {}", scheme);

        let default_host = format!("ws-{}.pusher.com", self.config.cluster);
        let host = self.config.host.as_deref().unwrap_or(&default_host);

        let url = format!(
            "{}://{}/app/{}?protocol=7",
            scheme, host, self.config.app_key
        );

        info!("WebSocket URL: {}", url);
        Url::parse(&url).map_err(PusherError::from)
    }

    fn generate_shared_secret(&self, channel_name: &str) -> Vec<u8> {
        let mut hmac = Hmac::<Sha256>::new_from_slice(self.config.app_secret.as_bytes())
            .expect("HMAC can take key of any size");
        hmac.update(channel_name.as_bytes());
        hmac.finalize().into_bytes().to_vec()
    }

    fn encrypt_data(&self, data: &str, shared_secret: &[u8]) -> PusherResult<String> {
        let iv = rand::thread_rng().gen::<[u8; 16]>();
        let cipher = Encryptor::<Aes256>::new(shared_secret.into(), &iv.into());

        let plaintext = data.as_bytes();
        let mut buffer = vec![0u8; plaintext.len() + 16]; // Add space for padding
        buffer[..plaintext.len()].copy_from_slice(plaintext);

        let ciphertext_len = cipher
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
            .map_err(|e| PusherError::EncryptionError(e.to_string()))?
            .len();

        let mut result = iv.to_vec();
        result.extend_from_slice(&buffer[..ciphertext_len]);

        Ok(STANDARD.encode(result))
    }


    /// Gets the current connection state.
    ///
    /// # Returns
    ///
    /// The current `ConnectionState`.
    pub async fn get_connection_state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    /// Gets a list of currently subscribed channels.
    ///
    /// # Returns
    ///
    /// A vector of channel names.
    pub async fn get_subscribed_channels(&self) -> Vec<String> {
        self.channels.read().await.keys().cloned().collect()
    }

    /// Sends a test event through the client.
    ///
    /// # Arguments
    ///
    /// * `event` - The event to send.
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn send_test_event(&self, event: Event) -> PusherResult<()> {
        self.event_tx
            .send(event)
            .await
            .map_err(|e| PusherError::WebSocketError(e.to_string()))
    }

    /// Gets the current socket ID if connected, or None if not connected.
    ///
    /// # Returns
    ///
    /// A `PusherResult` containing the socket ID string if connected, or None if not connected.
    pub async fn get_socket_id(&self) -> PusherResult<Option<String>> {
        Ok(self.socket_id.read().await.clone())
    }

    /// Binds a callback to be executed when the client connects to Pusher.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function to be called when the connection is established
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn on_connect<F>(&self, callback: F) -> PusherResult<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.bind("pusher:connection_established", move |_| {
            callback();
        })
        .await
    }

    /// Binds a callback to be executed when the client disconnects from Pusher.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function to be called when the connection is lost
    ///
    /// # Returns
    ///
    /// A `PusherResult` indicating success or failure.
    pub async fn on_disconnect<F>(&self, callback: F) -> PusherResult<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.bind("pusher:disconnected", move |_| {
            callback();
        })
        .await
    }

    /// Checks if the client is currently connected to Pusher.
    ///
    /// # Returns
    ///
    /// `true` if the client is connected, `false` otherwise.
    pub async fn is_connected(&self) -> bool {
        matches!(self.get_connection_state().await, ConnectionState::Connected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation() {
        let config =
            PusherConfig::from_env().expect("Failed to load Pusher configuration from environment");
        let client = PusherClient::new(config).unwrap();
        assert_eq!(*client.state.read().await, ConnectionState::Disconnected);
    }

    #[tokio::test]
    #[ignore]
    async fn test_generate_shared_secret() {
        let config =
            PusherConfig::from_env().expect("Failed to load Pusher configuration from environment");
        let client = PusherClient::new(config).unwrap();
        let secret = client.generate_shared_secret("test-channel");
        assert!(!secret.is_empty());
    }

    #[tokio::test]
    async fn test_trigger_batch() {
        let config =
            PusherConfig::from_env().expect("Failed to load Pusher configuration from environment");
        let client = PusherClient::new(config).unwrap();

        let batch_events = vec![
            BatchEvent {
                channel: "test-channel-1".to_string(),
                event: "test-event-1".to_string(),
                data: "{\"message\": \"Hello from event 1\"}".to_string(),
            },
            BatchEvent {
                channel: "test-channel-2".to_string(),
                event: "test-event-2".to_string(),
                data: "{\"message\": \"Hello from event 2\"}".to_string(),
            },
        ];

        let result = client.trigger_batch(batch_events).await;
        assert!(result.is_ok());
    }
}
