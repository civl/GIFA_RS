//! 识别结果：两个通道**各自独立**，绝不合成为一个角度。
//!
//! 原版反复强调：站着不动把镜头转 180°，人没动、视角转了 —— 所以
//! 「人往哪走」和「镜头朝哪看」是两个量，滤波与门控也必须分开。

/// 行进方向（青色三角解出来的）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrow {
    /// 罗盘角（0°=北，顺时针）
    pub angle: f64,
    /// 0..=1
    pub confidence: f64,
    /// 青色像素数（诊断）
    pub pixels: u32,
    /// 三点几何的钝度（越高越像"鼻尖"）
    pub nose_flatness: f64,
}

/// 镜头视角（白色扇形解出来的）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cone {
    pub angle: f64,
    pub confidence: f64,
}

/// 一帧的完整结论
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bearing {
    /// `None` = 这一帧没识别到箭头（不要拿视角顶替）
    pub arrow: Option<Arrow>,
    /// `None` = 这一帧没有视锥证据
    pub cone: Option<Cone>,
    /// 画面里有没有小地图（门控）
    pub map_present: bool,
}

impl Bearing {
    /// 送给叠加层的东西：只带"能画"的量
    pub fn is_drawable(&self) -> bool {
        self.arrow.is_some() || self.cone.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bearing_is_not_drawable() {
        let b = Bearing::default();
        assert!(!b.is_drawable());
        assert!(!b.map_present);
    }

    #[test]
    fn cone_alone_is_drawable_but_has_no_arrow() {
        let b = Bearing {
            arrow: None,
            cone: Some(Cone {
                angle: 90.0,
                confidence: 0.6,
            }),
            map_present: true,
        };
        assert!(b.is_drawable());
        assert!(b.arrow.is_none(), "视角不能冒充行进方向");
    }
}
