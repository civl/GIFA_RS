//! 快捷键加速器：解析、分流（系统热键 vs 低级钩子）、冲突交换。
//!
//! 分流规则照搬原版 `hook-hotkeys.js::isHookAccelerator`：
//! * 鼠标侧键 —— `RegisterHotKey` 不支持鼠标
//! * 不带修饰键的单键 —— 注册后会**全局吞掉**该键，用户在任何程序里都打不出来
//! * 按住类动作（`holdSpeed`）、循环类动作（`cycleSpeed`）—— 需要配对按下/松开或去重

use crate::action::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mod {
    Ctrl = 1,
    Alt = 2,
    Shift = 4,
    Super = 8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accelerator {
    pub mods: u8,
    /// 规范化后的键名（小写）：`space` / `left` / `f8` / `mouse4` / `a`
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    /// 只有修饰键，没有主键（`Ctrl+`）
    NoKey,
    /// 修饰键重复（`Ctrl+Ctrl+A`）
    DuplicateMod,
    /// 认不出的修饰键
    UnknownMod(String),
}

fn parse_mod(part: &str) -> Option<Mod> {
    Some(match part.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "commandorcontrol" | "cmdorctrl" => Mod::Ctrl,
        "alt" | "option" => Mod::Alt,
        "shift" => Mod::Shift,
        "super" | "meta" | "command" | "cmd" | "win" | "windows" => Mod::Super,
        _ => return None,
    })
}

impl Accelerator {
    pub fn parse(raw: &str) -> Result<Self, ParseError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ParseError::Empty);
        }
        let parts: Vec<&str> = raw.split('+').map(str::trim).collect();
        let mut mods = 0u8;
        for part in &parts[..parts.len() - 1] {
            match parse_mod(part) {
                Some(m) => {
                    if mods & (m as u8) != 0 {
                        return Err(ParseError::DuplicateMod);
                    }
                    mods |= m as u8;
                }
                None => return Err(ParseError::UnknownMod(part.to_string())),
            }
        }
        let key = parts[parts.len() - 1];
        if key.is_empty() || parse_mod(key).is_some() {
            return Err(ParseError::NoKey);
        }
        Ok(Self {
            mods,
            key: key.to_ascii_lowercase(),
        })
    }

    pub fn has(&self, m: Mod) -> bool {
        self.mods & (m as u8) != 0
    }

    /// 是否必须走低级钩子（被动监听）而不是系统热键（独占注册）
    pub fn needs_hook(&self, action: Action) -> bool {
        action.is_hold()
            || matches!(action, Action::CycleSpeed)
            || self.key.starts_with("mouse")
            || self.mods == 0
    }

    /// 界面上的可读写法
    pub fn describe(&self) -> String {
        match self.key.as_str() {
            "mouse4" => return "鼠标侧键（后退）".into(),
            "mouse5" => return "鼠标侧键（前进）".into(),
            _ => {}
        }
        let mut out = String::new();
        for (bit, name) in [
            (Mod::Ctrl, "Ctrl"),
            (Mod::Alt, "Alt"),
            (Mod::Shift, "Shift"),
            (Mod::Super, "Win"),
        ] {
            if self.has(bit) {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(&self.key);
        out
    }
}

/// 改键计划：把 `action` 绑到 `accelerator`，必要时与旧占用者交换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapPlan {
    /// 与目标键冲突、需要让位的动作 → 它会拿到 `action` 原来的键
    pub displaced: Vec<(Action, String)>,
    /// 最终要绑的（action, accelerator）
    pub bind: (Action, String),
}

/// 计算交换计划。
///
/// 原版语义：新键若正被别的动作占用，两者**交换**；如果交换后任一动作注册失败，
/// 整体回滚（注册在运行期做，这里只产出计划）。
pub fn plan_swap(
    action: Action,
    accelerator: &str,
    current: &[(Action, String)],
) -> Result<SwapPlan, ParseError> {
    Accelerator::parse(accelerator)?;
    let target = accelerator.trim().to_string();
    let own_old = current
        .iter()
        .find(|(a, _)| *a == action)
        .map(|(_, s)| s.clone());

    let mut displaced = Vec::new();
    for (other, acc) in current {
        if *other == action {
            continue;
        }
        if acc == &target {
            if let Some(old) = own_old.as_ref() {
                displaced.push((*other, old.clone()));
            } else {
                // 目标动作原本没绑定 → 对方只能被清空
                displaced.push((*other, String::new()));
            }
        }
    }
    Ok(SwapPlan {
        displaced,
        bind: (action, target),
    })
}

/// 把「动作 → 加速器」的整张表在纯数据层做交换，返回新表。
///
/// 与 `plan_swap` 的区别：这里直接给出可以落盘/重新注册的完整映射，
/// 注册失败时调用方用旧表回滚即可（事务性由上层保证）。
pub fn swap_map(
    current: &std::collections::BTreeMap<String, String>,
    action: Action,
    accelerator: &str,
) -> Result<std::collections::BTreeMap<String, String>, ParseError> {
    Accelerator::parse(accelerator)?;
    let target = accelerator.trim().to_string();
    let mut next = current.clone();
    let own_old = next.get(action.key()).cloned();

    // 找到占用目标键的其它动作
    let owners: Vec<String> = next
        .iter()
        .filter(|(k, v)| k.as_str() != action.key() && v.as_str() == target)
        .map(|(k, _)| k.clone())
        .collect();

    for owner_key in owners {
        match own_old.as_ref() {
            Some(old) if !old.trim().is_empty() => {
                next.insert(owner_key, old.clone());
            }
            _ => {
                next.remove(&owner_key);
            }
        }
    }
    next.insert(action.key().to_string(), target);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combinations() {
        let a = Accelerator::parse("Alt+Space").unwrap();
        assert!(a.has(Mod::Alt));
        assert_eq!(a.key, "space");

        let none = Accelerator::parse("F8").unwrap();
        assert_eq!(none.mods, 0);
        assert_eq!(none.key, "f8");

        let mouse = Accelerator::parse("Mouse4").unwrap();
        assert_eq!(mouse.key, "mouse4");
        assert_eq!(mouse.describe(), "鼠标侧键（后退）");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Accelerator::parse(""), Err(ParseError::Empty));
        assert_eq!(Accelerator::parse("Ctrl+"), Err(ParseError::NoKey));
        assert_eq!(
            Accelerator::parse("Ctrl+Ctrl+A"),
            Err(ParseError::DuplicateMod)
        );
        assert!(matches!(
            Accelerator::parse("Hyper+A"),
            Err(ParseError::UnknownMod(_))
        ));
    }

    #[test]
    fn hook_routing_matches_original() {
        let alt_space = Accelerator::parse("Alt+Space").unwrap();
        assert!(!alt_space.needs_hook(Action::TogglePlay));
        assert!(alt_space.needs_hook(Action::CycleSpeed));

        let f8 = Accelerator::parse("F8").unwrap();
        assert!(f8.needs_hook(Action::HoldSpeed));

        let single = Accelerator::parse("J").unwrap();
        assert!(single.needs_hook(Action::TogglePlay), "单键必须走钩子");

        let mouse = Accelerator::parse("Mouse4").unwrap();
        assert!(mouse.needs_hook(Action::SeekBack));
    }

    #[test]
    fn plan_swaps_conflicting_binding() {
        let current = vec![
            (Action::TogglePlay, "Alt+Space".to_string()),
            (Action::SeekFwd, "Alt+Right".to_string()),
        ];
        let plan = plan_swap(Action::SeekFwd, "Alt+Space", &current).unwrap();
        assert_eq!(
            plan.displaced,
            vec![(Action::TogglePlay, "Alt+Right".to_string())]
        );
        assert_eq!(plan.bind, (Action::SeekFwd, "Alt+Space".to_string()));
    }

    #[test]
    fn plan_without_conflict_only_binds() {
        let current = vec![(Action::TogglePlay, "Alt+Space".to_string())];
        let plan = plan_swap(Action::SeekBack, "Alt+Left", &current).unwrap();
        assert!(plan.displaced.is_empty());
    }

    #[test]
    fn swap_map_exchanges_two_actions() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("togglePlay".to_string(), "Alt+Space".to_string());
        m.insert("seekFwd".to_string(), "Alt+Right".to_string());

        let next = swap_map(&m, Action::SeekFwd, "Alt+Space").unwrap();
        assert_eq!(next["seekFwd"], "Alt+Space");
        assert_eq!(next["togglePlay"], "Alt+Right", "被占用的动作拿到旧键");
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn swap_map_clears_when_target_action_was_unbound() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("togglePlay".to_string(), "Alt+Space".to_string());
        // seekBack 原本没绑定 → 占用者只能被清空
        let next = swap_map(&m, Action::SeekBack, "Alt+Space").unwrap();
        assert_eq!(next["seekBack"], "Alt+Space");
        assert!(!next.contains_key("togglePlay"));
    }

    #[test]
    fn swap_map_rejects_bad_accelerator() {
        let m = std::collections::BTreeMap::new();
        assert_eq!(
            swap_map(&m, Action::TogglePlay, "Ctrl+"),
            Err(ParseError::NoKey)
        );
    }

    #[test]
    fn describe_orders_modifiers() {
        assert_eq!(
            Accelerator::parse("Shift+Ctrl+Alt+Q").unwrap().describe(),
            "Ctrl+Alt+Shift+q"
        );
    }
}
