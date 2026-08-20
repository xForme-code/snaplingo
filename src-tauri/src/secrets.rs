//! 凭据存储：API Key 之类的东西不进 config.json。
//!
//! 为什么值得单独做一层：config.json 是明文的，用户会因为「改个快捷键」
//! 「看看配置在哪」而打开它，也会在提 issue 时把它整个贴出来——那一贴
//! 就把 OpenAI/Claude 的 Key 一起贴出去了。备份工具、同步盘同理。
//! macOS 有 Keychain 这个现成的地方，就不该让密钥躺在明文文件里。
//!
//! 其它平台暂时仍走 config.json（Windows 有 DPAPI / Credential Manager，
//! Linux 有 Secret Service，但都得各写一套，先不铺开）。`available()`
//! 就是给调用方判断「这台机器上密钥到底存哪」用的。

/// Keychain 里的 service 名。**不要改**：改了等于老用户的密钥全部找不回来，
/// 界面上会显示成空，用户以为自己的 Key 丢了。
///
/// 留了个环境变量口子只为测试：测试要真的读写钥匙串才有意义，但绝不能
/// 动到用户自己那份 Key——换个 service 名就互不干扰。
#[cfg(target_os = "macos")]
fn service() -> String {
    std::env::var("SNAPLINGO_KEYCHAIN_SERVICE").unwrap_or_else(|_| "SnapLingo".into())
}

/// 这台机器上是否有独立的凭据存储。false 表示密钥还在 config.json 里。
pub fn available() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(target_os = "macos")]
pub fn load(account: &str) -> Option<String> {
    match security_framework::passwords::get_generic_password(&service(), account) {
        Ok(bytes) => String::from_utf8(bytes).ok(),
        Err(err) => {
            // 没存过是正常情况（用户没填这个引擎的 Key），不要当错误刷日志
            if err.code() != ERR_SEC_ITEM_NOT_FOUND {
                log::warn!("读取 Keychain 条目 {account} 失败: {err}");
            }
            None
        }
    }
}

/// 写入或删除。空值当作删除——用户把输入框清空，就是要撤掉这个 Key，
/// 留一个空条目在 Keychain 里没有意义。
#[cfg(target_os = "macos")]
pub fn store(account: &str, value: &str) -> anyhow::Result<()> {
    use security_framework::passwords as kc;

    if value.is_empty() {
        return match kc::delete_generic_password(&service(), account) {
            // 本来就没有，等同于删成功
            Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            other => Ok(other?),
        };
    }
    kc::set_generic_password(&service(), account, value.as_bytes())?;
    Ok(())
}

/// errSecItemNotFound。Security.framework 的「查无此条目」，不是故障。
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(not(target_os = "macos"))]
pub fn load(_account: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn store(_account: &str, _value: &str) -> anyhow::Result<()> {
    Ok(())
}

// Keychain 的行为没法用假对象验证——要么真的写进去再读出来，要么等于没测。
// 所以这组测试打了 #[ignore]，默认不跑（CI 上没有可用的钥匙串，跑了必失败），
// 改动本文件后手动跑：cargo test --lib -- --ignored keychain
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{load, store};

    const ACCOUNT: &str = "__snaplingo_test__";

    #[test]
    #[ignore = "会真的读写登录钥匙串"]
    fn keychain_round_trip() {
        store(ACCOUNT, "sk-test-12345").expect("写入失败");
        assert_eq!(load(ACCOUNT).as_deref(), Some("sk-test-12345"));

        // 覆盖写：同一个 account 应该是更新而不是报「已存在」
        store(ACCOUNT, "sk-test-67890").expect("覆盖失败");
        assert_eq!(load(ACCOUNT).as_deref(), Some("sk-test-67890"));

        // 空值 = 删除
        store(ACCOUNT, "").expect("删除失败");
        assert_eq!(load(ACCOUNT), None);

        // 删不存在的条目不该报错
        store(ACCOUNT, "").expect("重复删除应当等同成功");
    }
}
