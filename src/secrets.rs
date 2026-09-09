use anyhow::{Result, anyhow};
use secret_service::{EncryptionType, SecretService};
use std::collections::HashMap;

pub struct SecretStore;
impl SecretStore {
    pub async fn set(server: &str, name: &str, value: &str) -> Result<()> {
        let service = SecretService::connect(EncryptionType::Dh).await?;
        let collection = service.get_default_collection().await?;
        collection.unlock().await?;
        collection
            .create_item(
                &format!("Marshal: {name}"),
                Self::attributes(server, name),
                value.as_bytes(),
                true,
                "text/plain",
            )
            .await?;
        Ok(())
    }
    pub async fn contains(server: &str, name: &str) -> Result<bool> {
        let service = SecretService::connect(EncryptionType::Dh).await?;
        let result = service.search_items(Self::attributes(server, name)).await?;
        Ok(!result.unlocked.is_empty() || !result.locked.is_empty())
    }
    pub async fn get(server: &str, name: &str) -> Result<String> {
        let service = SecretService::connect(EncryptionType::Dh).await?;
        let result = service.search_items(Self::attributes(server, name)).await?;
        let item = result
            .unlocked
            .first()
            .or(result.locked.first())
            .ok_or_else(|| anyhow!("Secret {name} is missing from the keyring"))?;
        item.unlock().await?;
        Ok(String::from_utf8(item.get_secret().await?)?)
    }
    pub async fn delete(server: &str, name: &str) -> Result<()> {
        let service = SecretService::connect(EncryptionType::Dh).await?;
        let result = service.search_items(Self::attributes(server, name)).await?;
        for item in result.unlocked.iter().chain(result.locked.iter()) {
            item.unlock().await?;
            item.delete().await?;
        }
        Ok(())
    }
    fn attributes<'a>(server: &'a str, name: &'a str) -> HashMap<&'a str, &'a str> {
        HashMap::from([
            ("application", "io.github.marshal.Marshal"),
            ("server", server),
            ("name", name),
        ])
    }
}
