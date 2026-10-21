use crate::{
    Core, Environment,
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
    /// Fetch a same-origin icon using only non-sensitive configured headers.
    pub async fn favicon_image(&self, id: &str, url: &str) -> Result<FaviconImage> {
        self.favicon_fetch(id, async |env| self.odoo.favicon_image(env, url).await)
            .await
    }

    /// Public display metadata using only non-sensitive configured headers.
    pub async fn favicon_page(&self, id: &str) -> Result<FaviconPage> {
        self.favicon_fetch(id, async |env| self.odoo.favicon_page(env).await)
            .await
    }

    async fn favicon_fetch<T>(
        &self,
        id: &str,
        fetch: impl AsyncFnOnce(&Environment) -> Result<T>,
    ) -> Result<T> {
        let (env, session) = {
            let state = self.inner.lock().await;
            let data = state.vault.as_ref().ok_or(OdxError::Locked)?.data();
            let mut env = data.connection_environment(id)?;
            env.headers
                .retain(|h| !crate::headers::is_sensitive(&h.name));
            (env, state.session)
        };
        let _permit = FAVICON_PERMITS
            .acquire()
            .await
            .map_err(|_| OdxError::Capacity)?;
        self.assert_environment_current(id, env.revision, session)
            .await?;
        let value = fetch(&env).await?;
        self.assert_environment_current(id, env.revision, session)
            .await?;
        Ok(value)
    }
}
