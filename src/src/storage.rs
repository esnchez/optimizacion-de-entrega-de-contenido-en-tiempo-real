//! Thin wrapper around the AWS S3 client: the only module that talks to S3.

use std::{sync::Arc, time::Duration};

use aws_sdk_s3::{
    Client,
    config::{Region, http::HttpResponse},
    error::{DisplayErrorContext, SdkError},
    presigning::PresigningConfig,
    primitives::{ByteStream, DateTime, DateTimeFormat},
};
use bytes::Bytes;

use crate::{config::Config, error::AppError};

/// What we need to know about an object to answer HEAD, conditional and cached requests.
#[derive(Debug, Clone)]
pub struct ObjectMeta {
    pub size: u64,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    /// Already formatted as an HTTP date, ready to go into `Last-Modified`.
    pub last_modified: Option<String>,
}

/// A streaming GET from S3; the body is pulled lazily as the client reads.
pub struct ObjectStream {
    pub body: ByteStream,
    pub content_length: Option<u64>,
    /// Set when S3 answered a ranged request (`206 Partial Content`).
    pub content_range: Option<String>,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Clone)]
pub struct S3Store {
    client: Client,
    bucket: Arc<str>,
}

impl S3Store {
    /// Builds the client from the default AWS provider chain (env vars, IRSA/web identity on
    /// EKS, profile files, IMDS), with optional endpoint override for MinIO/LocalStack.
    pub async fn from_config(cfg: &Config) -> Self {
        let mut loader = aws_config::from_env();
        if let Some(region) = &cfg.aws_region {
            loader = loader.region(Region::new(region.clone()));
        }
        let shared = loader.load().await;

        let mut builder = aws_sdk_s3::config::Builder::from(&shared);
        if let Some(endpoint) = &cfg.s3_endpoint {
            builder = builder.endpoint_url(endpoint).force_path_style(true);
        }
        Self::new(Client::from_conf(builder.build()), &cfg.s3_bucket)
    }

    pub fn new(client: Client, bucket: &str) -> Self {
        Self {
            client,
            bucket: bucket.into(),
        }
    }

    /// `HeadObject`. A missing object is `Ok(None)` so callers can cache the negative result.
    pub async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, AppError> {
        let result = self
            .client
            .head_object()
            .bucket(&*self.bucket)
            .key(key)
            .send()
            .await;
        match result {
            Ok(out) => Ok(Some(ObjectMeta {
                size: out.content_length().unwrap_or(0).max(0) as u64,
                content_type: out.content_type().map(str::to_owned),
                etag: out.e_tag().map(str::to_owned),
                last_modified: out.last_modified().and_then(http_date),
            })),
            Err(err) if status_of(&err) == Some(404) => Ok(None),
            Err(err) => Err(map_sdk_error(err)),
        }
    }

    /// Streaming `GetObject`. `range` is the raw client `Range` header, forwarded as is.
    pub async fn get(&self, key: &str, range: Option<&str>) -> Result<ObjectStream, AppError> {
        let out = self
            .client
            .get_object()
            .bucket(&*self.bucket)
            .key(key)
            .set_range(range.map(str::to_owned))
            .send()
            .await
            .map_err(map_sdk_error)?;

        Ok(ObjectStream {
            content_length: out.content_length().map(|len| len.max(0) as u64),
            content_range: out.content_range().map(str::to_owned),
            content_type: out.content_type().map(str::to_owned),
            etag: out.e_tag().map(str::to_owned),
            last_modified: out.last_modified().and_then(http_date),
            body: out.body,
        })
    }

    /// Downloads a whole object into memory. Only used for small objects.
    pub async fn get_bytes(&self, key: &str) -> Result<Bytes, AppError> {
        let stream = self.get(key, None).await?;
        let data = stream
            .body
            .collect()
            .await
            .map_err(|err| AppError::Storage(err.to_string()))?;
        Ok(data.into_bytes())
    }

    /// Presigned GET URL so clients can download straight from S3.
    pub async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, AppError> {
        let presign_cfg =
            PresigningConfig::expires_in(ttl).map_err(|err| AppError::Storage(err.to_string()))?;
        let request = self
            .client
            .get_object()
            .bucket(&*self.bucket)
            .key(key)
            .presigned(presign_cfg)
            .await
            .map_err(map_sdk_error)?;
        Ok(request.uri().to_owned())
    }
}

fn http_date(dt: &DateTime) -> Option<String> {
    dt.fmt(DateTimeFormat::HttpDate).ok()
}

fn status_of<E>(err: &SdkError<E, HttpResponse>) -> Option<u16> {
    err.raw_response().map(|resp| resp.status().as_u16())
}

fn map_sdk_error<E>(err: SdkError<E, HttpResponse>) -> AppError
where
    E: std::error::Error + 'static,
{
    match status_of(&err) {
        Some(404) => AppError::NotFound,
        Some(416) => AppError::RangeNotSatisfiable,
        _ => AppError::Storage(DisplayErrorContext(&err).to_string()),
    }
}
