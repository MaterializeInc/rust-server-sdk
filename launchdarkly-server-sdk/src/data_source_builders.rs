use super::service_endpoints;
use crate::data_source::{DataSource, NullDataSource, PollingDataSource, StreamingDataSource};
use crate::feature_requester_builders::{FeatureRequesterFactory, HyperFeatureRequesterBuilder};
use hyper::{client::connect::Connection, service::Service, Uri};
#[cfg(feature = "rustls")]
use hyper_rustls::HttpsConnectorBuilder;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(test)]
use super::data_source;

/// Error type used to represent failures when building a DataSource instance.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum BuildError {
    /// Error used when a configuration setting is invalid. This typically indicates an invalid URL.
    #[error("data source factory failed to build: {0}")]
    InvalidConfig(String),
}

const DEFAULT_INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MINIMUM_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Trait which allows creation of data sources. Should be implemented by data source builder types.
pub trait DataSourceFactory {
    fn build(
        &self,
        endpoints: &service_endpoints::ServiceEndpoints,
        sdk_key: &str,
        tags: Option<String>,
    ) -> Result<Arc<dyn DataSource>, BuildError>;
    fn to_owned(&self) -> Box<dyn DataSourceFactory>;
}

/// Contains methods for configuring the streaming data source.
///
/// By default, the SDK uses a streaming connection to receive feature flag data from LaunchDarkly. If you want
/// to customize the behavior of the connection, create a builder [StreamingDataSourceBuilder::new],
/// change its properties with the methods of this class, and pass it to
/// [crate::ConfigBuilder::data_source].
///
/// # Examples
///
/// Adjust the initial reconnect delay.
/// ```
/// # use launchdarkly_server_sdk::{StreamingDataSourceBuilder, ConfigBuilder};
/// # use hyper_rustls::HttpsConnector;
/// # use hyper::client::HttpConnector;
/// # use std::time::Duration;
/// # fn main() {
///     ConfigBuilder::new("sdk-key").data_source(StreamingDataSourceBuilder::<hyper_rustls::HttpsConnector<HttpConnector>>::new()
///         .initial_reconnect_delay(Duration::from_secs(10)));
/// # }
/// ```
#[derive(Clone)]
pub struct StreamingDataSourceBuilder<C> {
    initial_reconnect_delay: Duration,
    connector: Option<C>,
}

impl<C> StreamingDataSourceBuilder<C> {
    /// Create a new instance of the [StreamingDataSourceBuilder] with default values.
    pub fn new() -> Self {
        Self {
            initial_reconnect_delay: DEFAULT_INITIAL_RECONNECT_DELAY,
            connector: None,
        }
    }

    /// Sets the initial reconnect delay for the streaming connection.
    pub fn initial_reconnect_delay(&mut self, duration: Duration) -> &mut Self {
        self.initial_reconnect_delay = duration;
        self
    }

    /// Sets the connector for the event source client to use. This allows for re-use of a
    /// connector between multiple client instances. This is especially useful for the
    /// `sdk-test-harness` where many client instances are created throughout the test and reading
    /// the native certificates is a substantial portion of the runtime.
    pub fn https_connector(&mut self, connector: C) -> &mut Self {
        self.connector = Some(connector);
        self
    }
}

impl<C> DataSourceFactory for StreamingDataSourceBuilder<C>
where
    C: Service<Uri> + Clone + Send + Sync + 'static,
    C::Response: Connection + AsyncRead + AsyncWrite + Send + Unpin,
    C::Future: Send + 'static,
    C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    fn build(
        &self,
        endpoints: &service_endpoints::ServiceEndpoints,
        sdk_key: &str,
        tags: Option<String>,
    ) -> Result<Arc<dyn DataSource>, BuildError> {
        let data_source_result = match &self.connector {
            #[cfg(feature = "rustls")]
            None => {
                let connector = HttpsConnectorBuilder::new()
                    .with_native_roots()
                    .https_or_http()
                    .enable_http1()
                    .enable_http2()
                    .build();
                Ok(StreamingDataSource::new(
                    endpoints.streaming_base_url(),
                    sdk_key,
                    self.initial_reconnect_delay,
                    &tags,
                    connector,
                ))
            }
            #[cfg(not(feature = "rustls"))]
            None => Err(BuildError::InvalidConfig(
                "https connector required when rustls is disabled".into(),
            )),
            Some(connector) => Ok(StreamingDataSource::new(
                endpoints.streaming_base_url(),
                sdk_key,
                self.initial_reconnect_delay,
                &tags,
                connector.clone(),
            )),
        };
        let data_source = data_source_result?
            .map_err(|e| BuildError::InvalidConfig(format!("invalid stream_base_url: {:?}", e)))?;
        Ok(Arc::new(data_source))
    }

    fn to_owned(&self) -> Box<dyn DataSourceFactory> {
        Box::new(self.clone())
    }
}

impl<C> Default for StreamingDataSourceBuilder<C> {
    fn default() -> Self {
        StreamingDataSourceBuilder::new()
    }
}

#[derive(Clone)]
pub struct NullDataSourceBuilder {}

impl NullDataSourceBuilder {
    pub fn new() -> Self {
        Self {}
    }
}

impl DataSourceFactory for NullDataSourceBuilder {
    fn build(
        &self,
        _: &service_endpoints::ServiceEndpoints,
        _: &str,
        _: Option<String>,
    ) -> Result<Arc<dyn DataSource>, BuildError> {
        Ok(Arc::new(NullDataSource::new()))
    }

    fn to_owned(&self) -> Box<dyn DataSourceFactory> {
        Box::new(self.clone())
    }
}

impl Default for NullDataSourceBuilder {
    fn default() -> Self {
        NullDataSourceBuilder::new()
    }
}

/// Contains methods for configuring the polling data source.
///
/// Polling is not the default behavior; by default, the SDK uses a streaming connection to receive feature flag
/// data from LaunchDarkly. In polling mode, the SDK instead makes a new HTTP request to LaunchDarkly at regular
/// intervals. HTTP caching allows it to avoid redundantly downloading data if there have been no changes, but
/// polling is still less efficient than streaming and should only be used on the advice of LaunchDarkly support.
///
/// To use polling mode, create a builder [PollingDataSourceBuilder::new], change its properties
/// with the methods of this class, and pass it to the [crate::ConfigBuilder::data_source].
///
/// # Examples
///
/// Adjust the initial reconnect delay.
/// ```
/// # use launchdarkly_server_sdk::{PollingDataSourceBuilder, ConfigBuilder};
/// # use hyper_rustls::HttpsConnector;
/// # use hyper::client::HttpConnector;
/// # use std::time::Duration;
/// # fn main() {
///     ConfigBuilder::new("sdk-key").data_source(PollingDataSourceBuilder::<HttpsConnector<HttpConnector>>::new()
///         .poll_interval(Duration::from_secs(60)));
/// # }
/// ```
#[derive(Clone)]
pub struct PollingDataSourceBuilder<C> {
    poll_interval: Duration,
    connector: Option<C>,
}

/// Contains methods for configuring the polling data source.
///
/// Polling is not the default behavior; by default, the SDK uses a streaming connection to receive
/// feature flag data from LaunchDarkly. In polling mode, the SDK instead makes a new HTTP request
/// to LaunchDarkly at regular intervals. HTTP caching allows it to avoid redundantly downloading
/// data if there have been no changes, but polling is still less efficient than streaming and
/// should only be used on the advice of LaunchDarkly support.
///
/// To use polling mode, create a builder with [PollingDataSourceBuilder::new], set its properties
/// with the methods of this class, and pass it to [crate::ConfigBuilder::data_source].
///
/// # Examples
///
/// Adjust the poll interval.
/// ```
/// # use launchdarkly_server_sdk::{PollingDataSourceBuilder, ConfigBuilder};
/// # use std::time::Duration;
/// # use hyper_rustls::HttpsConnector;
/// # use hyper::client::HttpConnector;
/// # fn main() {
///     ConfigBuilder::new("sdk-key").data_source(PollingDataSourceBuilder::<HttpsConnector<HttpConnector>>::new()
///         .poll_interval(Duration::from_secs(60)));
/// # }
/// ```
impl<C> PollingDataSourceBuilder<C> {
    /// Create a new instance of the [PollingDataSourceBuilder] with default values.
    pub fn new() -> Self {
        Self {
            poll_interval: MINIMUM_POLL_INTERVAL,
            connector: None,
        }
    }

    /// Sets the poll interval for the polling connection.
    ///
    /// The default and minimum value is 30 seconds. Values less than this will be set to the
    /// default.
    pub fn poll_interval(&mut self, poll_interval: Duration) -> &mut Self {
        self.poll_interval = std::cmp::max(poll_interval, MINIMUM_POLL_INTERVAL);
        self
    }

    /// Sets the connector for the polling client to use. This allows for re-use of a connector
    /// between multiple client instances. This is especially useful for the `sdk-test-harness`
    /// where many client instances are created throughout the test and reading the native
    /// certificates is a substantial portion of the runtime.
    pub fn https_connector(&mut self, connector: C) -> &mut Self {
        self.connector = Some(connector);
        self
    }
}

impl<C> DataSourceFactory for PollingDataSourceBuilder<C>
where
    C: Service<Uri> + Clone + Send + Sync + 'static,
    C::Response: Connection + AsyncRead + AsyncWrite + Send + Unpin,
    C::Future: Send + Unpin + 'static,
    C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    fn build(
        &self,
        endpoints: &service_endpoints::ServiceEndpoints,
        sdk_key: &str,
        tags: Option<String>,
    ) -> Result<Arc<dyn DataSource>, BuildError> {
        let feature_requester_builder: Result<Box<dyn FeatureRequesterFactory>, BuildError> =
            match &self.connector {
                #[cfg(feature = "rustls")]
                None => {
                    let connector = HttpsConnectorBuilder::new()
                        .with_native_roots()
                        .https_or_http()
                        .enable_http1()
                        .enable_http2()
                        .build();

                    Ok(Box::new(HyperFeatureRequesterBuilder::new(
                        endpoints.polling_base_url(),
                        sdk_key,
                        connector,
                    )))
                }
                #[cfg(not(feature = "rustls"))]
                None => Err(BuildError::InvalidConfig(
                    "https connector required when rustls is disabled".into(),
                )),
                Some(connector) => Ok(Box::new(HyperFeatureRequesterBuilder::new(
                    endpoints.polling_base_url(),
                    sdk_key,
                    connector.clone(),
                ))),
            };

        let feature_requester_factory: Arc<Mutex<Box<dyn FeatureRequesterFactory>>> =
            Arc::new(Mutex::new(feature_requester_builder?));

        let data_source =
            PollingDataSource::new(feature_requester_factory, self.poll_interval, tags);
        Ok(Arc::new(data_source))
    }

    fn to_owned(&self) -> Box<dyn DataSourceFactory> {
        Box::new(self.clone())
    }
}

impl<C> Default for PollingDataSourceBuilder<C> {
    fn default() -> Self {
        PollingDataSourceBuilder::new()
    }
}

/// For testing you can use this builder to inject the MockDataSource.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct MockDataSourceBuilder {
    data_source: Option<Arc<data_source::MockDataSource>>,
}

#[cfg(test)]
impl MockDataSourceBuilder {
    pub fn new() -> MockDataSourceBuilder {
        MockDataSourceBuilder { data_source: None }
    }

    pub fn data_source(
        &mut self,
        data_source: Arc<data_source::MockDataSource>,
    ) -> &mut MockDataSourceBuilder {
        self.data_source = Some(data_source);
        self
    }
}

#[cfg(test)]
impl DataSourceFactory for MockDataSourceBuilder {
    fn build(
        &self,
        _endpoints: &service_endpoints::ServiceEndpoints,
        _sdk_key: &str,
        _tags: Option<String>,
    ) -> Result<Arc<dyn DataSource>, BuildError> {
        Ok(self.data_source.as_ref().unwrap().clone())
    }

    fn to_owned(&self) -> Box<dyn DataSourceFactory> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use hyper::client::HttpConnector;

    use super::*;

    #[test]
    fn default_stream_builder_has_correct_defaults() {
        let builder: StreamingDataSourceBuilder<HttpConnector> = StreamingDataSourceBuilder::new();

        assert_eq!(
            builder.initial_reconnect_delay,
            DEFAULT_INITIAL_RECONNECT_DELAY
        );
    }

    #[test]
    fn stream_builder_can_use_custom_connector() {
        #[derive(Debug, Clone)]
        struct TestConnector;
        impl hyper::service::Service<hyper::Uri> for TestConnector {
            type Response = tokio::net::TcpStream;
            type Error = std::io::Error;
            type Future = futures::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

            fn poll_ready(
                &mut self,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), Self::Error>> {
                std::task::Poll::Ready(Ok(()))
            }

            fn call(&mut self, _req: hyper::Uri) -> Self::Future {
                // this won't be called during the test
                unreachable!();
            }
        }

        let mut builder = StreamingDataSourceBuilder::new();
        builder.https_connector(TestConnector);
        assert!(builder
            .build(
                &crate::ServiceEndpointsBuilder::new().build().unwrap(),
                "test",
                None
            )
            .is_ok());
    }

    #[test]
    fn default_polling_builder_has_correct_defaults() {
        let builder = PollingDataSourceBuilder::<HttpConnector>::new();
        assert_eq!(builder.poll_interval, MINIMUM_POLL_INTERVAL,);
    }

    #[test]
    fn initial_reconnect_delay_for_streaming_can_be_adjusted() {
        let mut builder = StreamingDataSourceBuilder::<()>::new();
        builder.initial_reconnect_delay(Duration::from_secs(1234));
        assert_eq!(builder.initial_reconnect_delay, Duration::from_secs(1234));
    }
}
