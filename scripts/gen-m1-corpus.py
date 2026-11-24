"""生成把 m1 语料在编译期嵌入的模块，供 examples 使用。

为什么需要：本机 DSH 沙箱下，pwsh 直接启动的子进程（cargo 编译出的 exe）
无法读文件系统（os error 5），但 rustc 在**编译期**可以读。
所以用 include_bytes! 把语料嵌进二进制，运行期不碰文件。
"""
import pathlib
import re

repo = pathlib.Path(__file__).resolve().parent.parent
frames_dir = repo / "docs" / "calibration" / "m1-sub"
frames = sorted(frames_dir.glob("f*.bin"))

lines = [
    "// 自动生成（scripts/gen-m1-corpus.py），不要手改。",
    "// 把 m1 语料在**编译期**嵌入：本机沙箱下子进程读不了文件系统，",
    "// 但 rustc 编译期可以读，所以运行期完全不碰 fs。",
    'pub const LABELS: &[u8] = include_bytes!("../../../../docs/calibration/m1-labels.json");',
    "",
    "#[allow(clippy::type_complexity)]",
    "pub static FRAMES: &[(u64, &[u8])] = &[",
]
for f in frames:
    m = re.match(r"f(\d+)-(\d+)\.bin$", f.name)
    if not m:
        continue
    lines.append(f'    ({int(m.group(1))}, include_bytes!("../../../../docs/calibration/m1-sub/{f.name}")),')
lines.append("];")

out = repo / "crates" / "gifa-vision" / "examples" / "support" / "m1_corpus.rs"
out.write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"生成 {len(frames)} 帧 → {out}")
