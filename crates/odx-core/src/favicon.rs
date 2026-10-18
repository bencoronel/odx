use crate::{
    Core,
    error::{OdxError, Result},
};
use serde::Serialize;

/// Separate favicon concurrency keeps agent permits available; each download has a 5 s timeout.
static FAVICON_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Serialize)]
pub struct FaviconPage {
    pub url: String,
    pub html: String,
}

#[derive(Serialize)]
pub struct FaviconImage {
    pub content_type: String,
    pub data: Vec<u8>,
}

impl Core {
    pub async fn favicon_image(&self, id: &str, url: &str) -> Result<FaviconImage> {
        let (env, session) = {
            let state = self.inner.lock().await;
            let data = state.vault.as_ref().ok_or(OdxError::Locked)?.data();
            let env = data
                .environments
                .iter()
                .find(|e| e.id == id)
                .cloned()
                .ok_or(OdxError::EnvironmentNotFound)?;
            // Use the configured proxy without headers or credentials.
            let env = crate::config::Environment {
                proxy: data.effective_proxy(&env),
                ..env
            };
            (env, state.session)
        };
        let _permit = FAVICON_PERMITS
            .acquire()
            .await
            .map_err(|_| OdxError::Capacity)?;
        self.assert_environment_current(id, env.revision, session)
            .await?;
        let image = self.odoo.favicon_image(&env, url).await?;
        self.assert_environment_current(id, env.revision, session)
            .await?;
        Ok(image)
    }

    /// Public display metadata without vault keys, cookies or headers.
    pub async fn favicon_page(&self, id: &str) -> Result<FaviconPage> {
        let (env, session) = {
            let state = self.inner.lock().await;
            let data = state.vault.as_ref().ok_or(OdxError::Locked)?.data();
            let env = data
                .environments
                .iter()
                .find(|e| e.id == id)
                .cloned()
                .ok_or(OdxError::EnvironmentNotFound)?;
            // Use the configured proxy without headers or credentials.
            let env = crate::config::Environment {
                proxy: data.effective_proxy(&env),
                ..env
            };
            (env, state.session)
        };
        let _permit = FAVICON_PERMITS
            .acquire()
            .await
            .map_err(|_| OdxError::Capacity)?;
        self.assert_environment_current(id, env.revision, session)
            .await?;
        let page = self.odoo.favicon_page(&env).await?;
        self.assert_environment_current(id, env.revision, session)
            .await?;
        Ok(page)
    }
}
