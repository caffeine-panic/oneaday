use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime, State, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct PendingAppUpdate(Mutex<Option<Update>>);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum UpdateProxySettings {
    System,
    Manual { url: String },
    Disabled,
}

impl UpdateProxySettings {
    fn manual_url(&self) -> Result<Option<url::Url>, String> {
        let Self::Manual { url } = self else {
            return Ok(None);
        };
        let proxy =
            url::Url::parse(url).map_err(|_| "更新代理地址无效，请在设置中检查".to_owned())?;
        if !matches!(proxy.scheme(), "http" | "https") {
            return Err("更新代理仅支持 HTTP 或 HTTPS 地址".to_owned());
        }
        if !proxy.username().is_empty() || proxy.password().is_some() {
            return Err("更新代理地址不能包含用户名或密码".to_owned());
        }
        if proxy.path() != "/" || proxy.query().is_some() || proxy.fragment().is_some() {
            return Err("更新代理地址只能包含协议、主机和端口".to_owned());
        }
        Ok(Some(proxy))
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    published_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum AppUpdateEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        downloaded: u64,
        content_length: Option<u64>,
    },
    Finished,
}

#[tauri::command]
pub async fn check_for_app_update<R: Runtime>(
    app: AppHandle<R>,
    pending: State<'_, PendingAppUpdate>,
    proxy_settings: UpdateProxySettings,
) -> Result<Option<AppUpdateInfo>, String> {
    *pending
        .0
        .lock()
        .map_err(|_| "更新状态不可用，请重启应用后重试".to_owned())? = None;

    let builder = app.updater_builder().timeout(Duration::from_secs(30));
    let builder = match &proxy_settings {
        UpdateProxySettings::System => builder,
        UpdateProxySettings::Manual { .. } => {
            let proxy = proxy_settings
                .manual_url()?
                .expect("manual proxy URL should exist");
            builder.proxy(proxy)
        }
        UpdateProxySettings::Disabled => builder.no_proxy(),
    };
    let update = builder
        .build()
        .map_err(|error| format!("初始化更新检查失败：{error}"))?
        .check()
        .await
        .map_err(|error| format!("检查更新失败：{error}"))?;

    let info = update.as_ref().map(|update| AppUpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        published_at: update.date.map(|date| date.to_string()),
    });
    *pending
        .0
        .lock()
        .map_err(|_| "更新状态不可用，请重启应用后重试".to_owned())? = update;
    Ok(info)
}

#[tauri::command]
pub async fn install_app_update<R: Runtime>(
    app: AppHandle<R>,
    pending: State<'_, PendingAppUpdate>,
    on_event: Channel<AppUpdateEvent>,
) -> Result<(), String> {
    let update = pending
        .0
        .lock()
        .map_err(|_| "更新状态不可用，请重启应用后重试".to_owned())?
        .take()
        .ok_or_else(|| "没有待安装的更新，请重新检查".to_owned())?;

    let mut started = false;
    let mut downloaded = 0_u64;
    let result = update
        .download_and_install(
            |chunk_length, content_length| {
                if !started {
                    started = true;
                    let _ = on_event.send(AppUpdateEvent::Started { content_length });
                }
                downloaded = downloaded.saturating_add(chunk_length as u64);
                let _ = on_event.send(AppUpdateEvent::Progress {
                    downloaded,
                    content_length,
                });
            },
            || {
                let _ = on_event.send(AppUpdateEvent::Finished);
            },
        )
        .await;

    if let Err(error) = result {
        *pending
            .0
            .lock()
            .map_err(|_| "更新状态不可用，请重启应用后重试".to_owned())? = Some(update);
        return Err(format!("下载或安装更新失败：{error}"));
    }

    app.restart();
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AppUpdateEvent, AppUpdateInfo, UpdateProxySettings};

    #[test]
    fn update_metadata_and_progress_keep_the_frontend_contract() {
        let metadata = AppUpdateInfo {
            version: "0.3.0".to_owned(),
            current_version: "0.2.0".to_owned(),
            notes: Some("signed release".to_owned()),
            published_at: None,
        };
        assert_eq!(
            serde_json::to_value(metadata).expect("metadata should serialize"),
            json!({
                "version": "0.3.0",
                "currentVersion": "0.2.0",
                "notes": "signed release",
                "publishedAt": null,
            })
        );
        assert_eq!(
            serde_json::to_value(AppUpdateEvent::Progress {
                downloaded: 512,
                content_length: Some(1024),
            })
            .expect("progress should serialize"),
            json!({
                "event": "progress",
                "data": { "downloaded": 512, "contentLength": 1024 },
            })
        );
    }

    #[test]
    fn manual_update_proxy_rejects_credentials_and_non_http_schemes() {
        let credentials = UpdateProxySettings::Manual {
            url: "http://user:secret@127.0.0.1:7897/".to_owned(),
        };
        assert_eq!(
            credentials
                .manual_url()
                .expect_err("credentials must be rejected"),
            "更新代理地址不能包含用户名或密码"
        );

        let socks = UpdateProxySettings::Manual {
            url: "socks5://127.0.0.1:7897/".to_owned(),
        };
        assert_eq!(
            socks
                .manual_url()
                .expect_err("SOCKS is not compiled into the updater"),
            "更新代理仅支持 HTTP 或 HTTPS 地址"
        );
    }
}
