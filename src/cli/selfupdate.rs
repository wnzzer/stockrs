use anyhow::{anyhow, Result};
use self_update::update::UpdateStatus;

const REPO_OWNER: &str = "wnzzer";
const REPO_NAME: &str = "stockrs";

/// 从 GitHub Releases 检查并更新到最新版本。
/// self_update 用阻塞式 reqwest，会自建运行时，因此必须放到 spawn_blocking，
/// 否则会 panic：不能在 tokio 运行时内再启动一个运行时。
pub async fn run(check_only: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || do_update(check_only))
        .await
        .map_err(|e| anyhow!("自更新任务异常：{}", e))?
}

fn do_update(check_only: bool) -> Result<()> {
    let current = self_update::cargo_crate_version!();
    let update = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(REPO_NAME)
        .current_version(current)
        .show_download_progress(true)
        .no_confirm(true)
        .build()
        .map_err(|e| anyhow!("初始化自更新失败：{}", e))?;

    if check_only {
        let latest = update
            .get_latest_release()
            .map_err(|e| anyhow!("查询最新版本失败：{}", e))?;
        if self_update::version::bump_is_greater(current, &latest.version).unwrap_or(false) {
            println!("发现新版本：v{current} → v{}", latest.version);
            println!("运行 `stockrs self-update` 更新");
        } else {
            println!("已是最新版本 v{current}");
        }
        return Ok(());
    }

    match update
        .update_extended()
        .map_err(|e| anyhow!("更新失败：{}", e))?
    {
        UpdateStatus::UpToDate => println!("已是最新版本 v{current}"),
        UpdateStatus::Updated(release) => {
            println!("已更新：v{current} → {}", release.version);
        }
    }
    Ok(())
}
