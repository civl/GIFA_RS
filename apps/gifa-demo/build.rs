//! 资源注入：清单（管理员 / PerMonitorV2 / longPathAware）+ 图标 + 版本信息。
//!
//! 为什么清单里写 `requireAdministrator`：低级钩子只能收到"前台窗口完整性级别 ≤ 自己"的按键。
//! 游戏若以管理员运行，非提权的浮窗一个热键都收不到（见 `docs/HOTKEYS.md` 的 UIPI 一节）。
//!
//! **debug 构建不要求管理员**：否则本地自检一跑就弹 UAC，CI 直接卡死。
//! `GIFA_NO_ADMIN=1 cargo build --release` 也能出"不提权"的版本，方便测 release 二进制本身。
//!
//! 清单走链接器 `/MANIFESTINPUT`（不依赖任何工具）；图标与版本信息走 Windows SDK 的 `rc.exe`
//! （MSYS 的 windres 在本机预处理会挂）。两者都找不到时降级：清单仍在，图标/版本缺失。

use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "0.2.6";
const APP_NAME: &str = "GIFA 原神跟跑助手";

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-env-changed=GIFA_NO_ADMIN");
    println!("cargo:rerun-if-env-changed=GIFA_RC");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    /* 页面资源是 `include_str!` 编进去的。不在这里声明依赖的话，改了 CSS/JS
     * cargo 不会重编这个 crate —— 表现是"改了半天没生效"，极难察觉。 */
    for f in ["web/chrome.html", "web/app.js", "web/style.css"] {
        println!("cargo:rerun-if-changed={f}");
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let release = std::env::var("PROFILE").unwrap_or_default() == "release";
    let no_admin = std::env::var_os("GIFA_NO_ADMIN").is_some();
    let admin = release && !no_admin;
    let level = if admin {
        "requireAdministrator"
    } else {
        "asInvoker"
    };

    // ---- 清单（链接器直接嵌）
    let manifest = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="GIFA" version="{VERSION}.0" processorArchitecture="*"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{level}" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#
    );
    let manifest_path = out.join(format!("gifa-{level}.manifest"));
    write_or_reuse(&manifest_path, &manifest, "清单");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest_path.display()
    );
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='{level}' uiAccess='false'");
    println!("cargo:rustc-env=GIFA_MANIFEST_LEVEL={level}");

    // ---- 图标 + 版本信息（rc.exe）
    embed_icon_and_version(&out);
}

fn embed_icon_and_version(out: &Path) {
    // 自己的图标（由 assets/icon.svg 渲染而来），不再引用原版仓库里的那张
    let icon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icon.ico");
    if !icon.exists() {
        println!("cargo:warning=找不到 icon.ico，跳过图标与版本资源");
        return;
    }
    // rc 命令里反斜杠有意义，统一成正斜杠
    let icon_s = icon.display().to_string().replace('\\', "/");
    let (maj, min, pat) = parse_version(VERSION);

    let rc = format!(
        r#"1 ICON "{icon_s}"
1 VERSIONINFO
FILEVERSION {maj},{min},{pat},0
PRODUCTVERSION {maj},{min},{pat},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404B0"
    BEGIN
      VALUE "CompanyName", "GIFA 项目"
      VALUE "FileDescription", "{APP_NAME}"
      VALUE "FileVersion", "{VERSION}"
      VALUE "InternalName", "GIFA"
      VALUE "OriginalFilename", "GIFA.exe"
      VALUE "ProductName", "{APP_NAME}"
      VALUE "ProductVersion", "{VERSION}"
      VALUE "LegalCopyright", "GPL-3.0-only"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x804, 1200
  END
END
"#
    );
    let rc_path = out.join("gifa.rc");
    write_or_reuse(&rc_path, &rc, ".rc");
    let res = out.join("gifa.res");

    let Some(rc_exe) = find_rc() else {
        println!("cargo:warning=找不到 rc.exe（Windows SDK），跳过图标与版本资源");
        return;
    };
    // rc.exe 对中文按当前代码页解析：显式加 /c 65001 让它按 UTF-8 读
    match Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/c")
        .arg("65001")
        .arg(format!("/fo{}", res.display()))
        .arg(&rc_path)
        .output()
    {
        Ok(o) if o.status.success() => {
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        Ok(o) => println!(
            "cargo:warning=rc.exe 失败，跳过图标与版本资源：{}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => println!("cargo:warning=跑不动 rc.exe：{e}"),
    }
}

/// 找 rc.exe：环境变量 > Windows SDK 最新版本 > PATH
fn find_rc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GIFA_RC") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&kits)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    for v in versions.iter().rev() {
        let candidate = v.join("x64").join("rc.exe");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// 写入构建产物；目录只读（沙箱/受限构建环境）时，若文件已存在且内容一致则复用。
///
/// 为什么需要：DSH 沙箱等环境下，工作区里构建出的 exe（含 build script）对文件系统
/// 只有读权限 —— 创建、写入、删除全部 `os error 5`。此时只要人工预置与预期
/// **完全一致**的内容，构建就能继续（链接器只读这个文件）。
fn write_or_reuse(path: &Path, expected: &str, what: &str) {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == expected {
            return;
        }
    }
    if let Err(err) = std::fs::write(path, expected) {
        /* 沙箱等受限构建环境：工作区里构建出的 exe（含本 build script）对文件系统
         * 只读（创建/写入/删除全部 os error 5，甚至把预置文件读成空）。
         * 此时假定文件已被人工预置好 —— 链接器与 rc.exe 是 SDK 二进制、不受限，
         * 能读到真实内容；本脚本读到的"空文件"只是它自己的受限视图。
         * 后续步骤若真缺文件（rc/link），它们自己的错误处理会兜住。 */
        println!(
            "cargo:warning={what}写入失败（os error {}），假定 {} 已预置",
            err.raw_os_error().unwrap_or(0),
            path.display()
        );
    }
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut it = v.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}
