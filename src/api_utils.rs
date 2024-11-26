use reqwest::{Client};
use serde_json::Value;
use std::time::Duration;
use base64::{engine::general_purpose, Engine};
use futures::executor::block_on;
use std::future::Future;

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub api_url: String,
    pub auth_header: String,
}

impl AuthConfig {
    pub fn new(url: &str, username: &str, token: &str) -> AuthConfig {
        AuthConfig {
            api_url: format!("{}/api", url),
            auth_header: build_auth_header(username, token),
        }
    }
}

fn build_auth_header(username: &str, token: &str) -> String {
    let credentials = format!("{}:{}", username, token);
    let encoded_credentials = general_purpose::STANDARD.encode(credentials);
    format!("Token {}", encoded_credentials)
}

#[derive(Debug, Clone)]
struct ApiClient {
    auth: AuthConfig,
    client: Client,
    backoff: Duration,
    wait: Duration,
    retry_count: u8,
}

impl ApiClient {
    pub fn new(auth: AuthConfig, backoff: Duration, wait: Duration, retry_count: u8) -> Self {
        let client = Client::new();
        ApiClient {
            auth,
            client,
            backoff,
            wait,
            retry_count,
        }
    }

    pub fn send_file(&self, request: &str, file: &Vec<u8>) -> Result<String, String> {
        block_on(self.retry(|client| send_file(client, request, file)))
    }

    pub fn send_metadata(&self, request: &str, json_metadata: &Value) -> Result<(), String> {
        block_on(self.retry(|client| send_metadata(client, request, json_metadata)))
    }

    pub fn request_metadata(&self, request: &str, json_metadata: Option<Value>) -> Result<Value, String> {
        block_on(self.retry(|client| request_metadata(client, request, json_metadata)))
    }

    async fn retry<F, Fut, T>(&self, func: F) -> Result<T, String>
    where
        F: Fn(&ApiClient) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, String>> + Send,
        T: Send,
    {
        for attempt in 0..self.retry_count {
            match func(self).await {
                Ok(result) => return Ok(result),
                Err(e) if attempt < self.retry_count - 1 => {
                    tokio::time::sleep(self.backoff).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err("Max retry attempts reached".to_string())
    }

    pub fn request_exact_and_similar(&self, media_token: &String) -> (Option<u32>, Option<Vec<(u32, f32)>>) {
        let response = self.request_metadata(
            "/posts/reverse-search",
            Some(serde_json::json!({"contentToken": media_token}))
        ).unwrap();

        let json_data: Value = response;
    
        let exact_id = json_data.get("exactPost").and_then(|post| post.get("id")).and_then(|id| id.as_u64()).map(|id| id as u32);
    
        let similar_posts = json_data.get("similarPosts").and_then(|posts| {
            posts.as_array().map(|array| {
                array
                    .iter()
                    .filter_map(|post| {
                        let id = post.get("post").and_then(|p| p.get("id")).and_then(|id| id.as_u64()).map(|id| id as u32);
                        let distance = post.get("distance").and_then(|d| d.as_f64()).map(|d| d as f32);
                        if let (Some(id), Some(distance)) = (id, distance) {
                            Some((id, distance))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<(u32, f32)>>()
            })
        });
    
        (exact_id, similar_posts)
    }

}

pub async fn send_file(api_client: &ApiClient, request: &str, file: &Vec<u8>) -> Result<String, String> {
    let form = reqwest::multipart::Form::new().part("content", reqwest::multipart::Part::bytes(file.clone()));
    let response = api_client
        .client
        .post(format!("{}{}", api_client.auth.api_url, request))
        .header("Authorization", &api_client.auth.auth_header)
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json_response: Value = response.json().await.map_err(|e| e.to_string())?;
    if let Some(token) = json_response.get("token").and_then(|t| t.as_str()) {
        Ok(token.to_string())
    } else {
        Err("Token not found in response".to_string())
    }
}

pub async fn send_metadata(api_client: &ApiClient, request: &str, json_metadata: &Value) -> Result<(), String> {
    api_client
        .client
        .post(format!("{}{}", api_client.auth.api_url, request))
        .header("Authorization", &api_client.auth.auth_header)
        .json(&json_metadata)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn request_metadata(api_client: &ApiClient, request: &str, json_metadata: &Option<Value>) -> Result<Value, String> {
    let mut request_builder = api_client
        .client
        .get(format!("{}{}", api_client.auth.api_url, request))
        .header("Authorization", &api_client.auth.auth_header);

    if let Some(metadata) = json_metadata {
        request_builder = request_builder.json(&metadata);
    }

    let response = request_builder.send().await.map_err(|e| e.to_string())?;
    let json_response = response.json::<Value>().await.map_err(|e| e.to_string())?;
    Ok(json_response)
}
