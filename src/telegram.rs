use anyhow::{bail, Result};
use serde_json::json;

pub async fn send(token: &str, chat_id: &str, text: &str) -> Result<()> {
    // Telegram caps messages at 4096 chars.
    let text: String = text.chars().take(4000).collect();
    let resp = reqwest::Client::new()
        .post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .json(&json!({ "chat_id": chat_id, "text": text, "disable_web_page_preview": true }))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("Telegram {}: {}", resp.status(), resp.text().await.unwrap_or_default());
    }
    Ok(())
}
