//! `pory --uninstall`：把 pory 在系统上留下的东西清干净。
//!
//! ## 两类改动，两种待遇
//!
//! - **shell 集成 + 缓存**：默认清理、不询问。理由：它们都是**可再生**的
//!   （重装 `--init-shell` 就能重建，缓存丢了只是重走一次网络），
//!   留着反而是垃圾。
//! - **配置文件**：**问一次**（按 y 才删）。理由：里面可能有用户手填的 API Key、
//!   调过的 `order` 与语种 —— 那是用户的劳动成果，删掉不可恢复。
//!   这是本项目唯一一次交互式提问，值。
//!
//! ## 边界
//!
//! 只清理 **pory 自己装的位置**。用户用 `--print` 把脚本放到别处的，
//! pory 不知道在哪、也不去满硬盘找（输出里说明这一点）。
//! 二进制本身也不删（它可能由 cargo / 包管理器管，删法各不相同）—— 只给提示。
//!
//! ## 为什么直接打印而不返回文本
//!
//! 这是**交互式**命令：提问必须紧跟它前面的清理报告出现，否则用户会先看到
//! 问题、后看到上下文。攒进字符串由上层统一打印会让顺序错乱 ——
//! 所以这里边做边 `println!`（与其它元操作命令的形态不同，原因在此）。

use crate::error::{PoryError, Result};
use std::path::Path;

/// 执行卸载，边做边打印。
pub fn run() -> Result<()> {
    // ── 1. shell 集成（默认清理）──
    for line in crate::shell::remove_integration()? {
        println!("{line}");
    }

    // ── 2. 缓存（默认清理）──
    match crate::cache::Cache::clear() {
        Ok(Some(p)) => println!("✓ 已清空缓存：{}", p.display()),
        Ok(None) => println!("· 缓存本来就是空的"),
        // 缓存故障不是错误（与运行时同一条哲学）：报告并继续
        Err(e) => println!("⚠ 缓存清理失败（已跳过）：{e}"),
    }

    // ── 3. 配置文件（问一次）──
    let path = crate::config::Config::path()?;
    if !path.exists() {
        println!("· 没有配置文件");
    } else if confirm_delete(&path)? {
        std::fs::remove_file(&path)
            .map_err(|e| PoryError::Config(format!("删除 {} 失败：{e}", path.display())))?;
        // 目录空了就顺手收掉（它只属于 pory）
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
        println!("✓ 已删除配置文件：{}", path.display());
    } else {
        println!("· 保留配置文件：{}", path.display());
    }

    // ── 4. 收尾说明：二进制不在清理范围内 ──
    println!();
    println!("完成。二进制本身没有被删除：");
    println!("  由 cargo 装的：cargo uninstall pory");
    println!("  其他方式装的：删掉你在 PATH 里放的那个 pory 文件即可");
    println!("  （用 --print 自己放到别处的补全脚本，pory 不知道位置，也需要你自己删）");

    Ok(())
}

/// 询问是否删除配置文件。
///
/// **非交互环境（stdin 不是终端）一律回答否** —— 脚本里跑 `pory --uninstall`
/// 时静默删掉用户的 API Key 是不可接受的，安全侧永远选「保留」。
fn confirm_delete(path: &Path) -> Result<bool> {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        println!("（当前不是交互终端，配置文件按「保留」处理）");
        return Ok(false);
    }

    print!(
        "? 也删除配置文件 {}？填过的 api_key 与设置会一起丢失 [y/N] ",
        path.display()
    );
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Ok(false);
    }
    Ok(is_yes(&line))
}

/// 判断用户输入是否为肯定回答（y / yes，大小写不敏感）。
fn is_yes(input: &str) -> bool {
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只有 y / yes（任意大小写、带空白）算同意；回车、n、其他一律否
    #[test]
    fn 只有_y_算同意() {
        assert!(is_yes("y"));
        assert!(is_yes("Y"));
        assert!(is_yes(" yes \n"));
        assert!(is_yes("YES"));
        assert!(!is_yes(""));
        assert!(!is_yes("\n"));
        assert!(!is_yes("n"));
        assert!(!is_yes("no"));
        assert!(!is_yes("yeah"));
        assert!(!is_yes("1"));
    }
}
