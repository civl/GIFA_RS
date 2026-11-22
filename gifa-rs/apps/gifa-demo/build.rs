//! 清单注入 + 图标字节内嵌。
//!
//! 为什么清单里写 `requireAdministrator`：低级钩子只能收到"前台窗口完整性级别 ≤ 自己"的按键。
//! 游戏若以管理员运行，非提权的浮窗一个热键都收不到（见 `docs/HOTKEYS.md` 的 UIPI 一节）。
//!
//! **debug 构建不要求管理员**：否则本地自检一跑就弹 UAC，CI 直接卡死。
//! `GIFA_NO_ADMIN=1 cargo build --release` 也能出"不提权"的版本，方便测 release 二进制本身。
//!
//! 不用 windres/rc.exe：本机 MSYS 的 windres 预处理直接失败，而 `link.exe` 自带
//! `/MANIFESTINPUT` + `/MANIFESTUAC`，不依赖任何外部资源编译器。
//! 图标不走资源段，改成把 .ico 字节内嵌、运行时 `CreateIconFromResource`（见 `app_icon`）。

use std::path::PathBuf;

const VERSION: &str = "0.2.6";

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-env-changed=GIFA_NO_ADMIN");
    // 图标变了要重编（运行时读的是 include_bytes!）
    println!("cargo:rerun-if-changed=../../../reference/GIFA/build/icon.ico");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let release = std::env::var("PROFILE").unwrap_or_default() == "release";
    let no_admin = std::env::var_os("GIFA_NO_ADMIN").is_some();
    let admin = release && !no_admin;
    let level = if admin { "requireAdministrator" } else { "asInvoker" };

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
    let path = out.join(format!("gifa-{level}.manifest"));
    std::fs::write(&path, manifest).expect("写清单失败");

    let p = path.display().to_string();
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{p}");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTUAC:level='{level}' uiAccess='false'"
    );
    println!("cargo:rustc-env=GIFA_MANIFEST_LEVEL={level}");
}
