use std::sync::Arc;

use anyhow::anyhow;
use mastodon_async::{prelude::*, registration::Registered, scopes, Language, Result as MResult};
use serde_json as json;
use spdlog::prelude::*;
use teloxide::types::UserId;

use crate::{config, InstanceState};

pub struct Client {
    inst_state: Arc<InstanceState>,
}

impl Client {
    pub fn new(inst_state: Arc<InstanceState>) -> Self {
        Self { inst_state }
    }

    pub async fn login(&self, tg_user_id: UserId) -> anyhow::Result<LoginUser> {
        let login_user = self
            .load_login_user(tg_user_id)
            .await
            .map_err(|err| anyhow!("failed to query user login data: {err}"))?;
        Ok(login_user)
    }

    pub async fn auth_step_1(&self, domain: impl Into<String>) -> MResult<Registered> {
        let registration = Registration::new(domain)
            .client_name(config::PACKAGE.name)
            .scopes(Scopes::write(scopes::Write::Statuses))
            .build()
            .await?;

        // Make sure the url is not `None` so that we can directly unwrap it later
        registration.authorize_url()?;

        Ok(registration)
    }

    pub async fn auth_step_2(
        &self,
        reg: &Registered,
        tg_user_id: UserId,
        auth_code: impl AsRef<str>,
    ) -> anyhow::Result<LoginUser> {
        let login_user = LoginUser {
            inst: reg.complete(auth_code.as_ref()).await?,
            tg_user_id,
        };
        self.save_login_user(tg_user_id, &login_user)
            .await
            .map_err(|err| anyhow!("failed to save user login data: {err}"))?;
        Ok(login_user)
    }

    pub async fn revoke(&self, login_user: &LoginUser) -> anyhow::Result<()> {
        self.delete_login_user(login_user.tg_user_id).await
    }
}

impl Client {
    async fn save_login_user(
        &self,
        tg_user_id: UserId,
        login_user: &LoginUser,
    ) -> anyhow::Result<()> {
        let (tg_user_id, login_user_data) = (tg_user_id.0 as i64, login_user.serialize());

        sqlx::query!(
            r#"
INSERT OR REPLACE INTO login_users ( tg_user_id, mastodon_async_data )
VALUES ( ?1, ?2 )
        "#,
            tg_user_id,
            login_user_data
        )
        .execute(self.inst_state.db.pool())
        .await?;

        Ok(())
    }

    async fn load_login_user(&self, tg_user_id: UserId) -> anyhow::Result<LoginUser> {
        let tg_user_id_num = tg_user_id.0 as i64;

        let record = sqlx::query!(
            r#"
SELECT mastodon_async_data
FROM login_users
WHERE tg_user_id = ?1
        "#,
            tg_user_id_num,
        )
        .fetch_one(self.inst_state.db.pool())
        .await?;

        LoginUser::deserialize(record.mastodon_async_data, tg_user_id)
    }

    async fn delete_login_user(&self, tg_user_id: UserId) -> anyhow::Result<()> {
        let tg_user_id_num = tg_user_id.0 as i64;

        _ = sqlx::query!(
            r#"
DELETE FROM login_users
WHERE tg_user_id = ?1
        "#,
            tg_user_id_num,
        )
        .execute(self.inst_state.db.pool())
        .await?;

        Ok(())
    }
}

pub struct LoginUser {
    inst: Mastodon,
    tg_user_id: UserId,
}

impl LoginUser {
    pub fn domain(&self) -> &str {
        &self.inst.data.base
    }
    pub async fn post_status(&self, text: impl Into<String>) -> anyhow::Result<String> {
        let status = StatusBuilder::new()
            .status(text)
            .visibility(Visibility::Public)
            .language(Language::Eng)
            .build()?;

        let posted = self.inst.new_status(status).await?;
        let url = posted.url.unwrap_or_else(|| "*invisible*".to_string());

        info!("tg user '{}' status posted: {url}", self.tg_user_id);
        Ok(url)
    }
}

impl LoginUser {
    fn serialize(&self) -> String {
        json::to_string(&self.inst.data).unwrap()
    }

    fn deserialize(input: impl AsRef<str>, tg_user_id: UserId) -> anyhow::Result<Self> {
        let data: Data = json::from_str(input.as_ref())?;
        Ok(Self {
            inst: data.into(),
            tg_user_id,
        })
    }
}
