//! 蛟龙16K (MECHREVO GM6BG0Q) EC 控制中心
//!
//! 依赖内核原生 `uniwill-laptop` 驱动（force=1 加载）+ `ryzenadj`。
//! 只读监控直接从 sysfs 读；所有写操作经 `pkexec` 提权（会弹 polkit 密码框）。

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

const APP_ID: &str = "cn.lidream.JiaolongCtl";
const HELPER: &str = "/usr/local/libexec/jiaolong-helper";

// ---------------------------------------------------------------- sysfs 基础

fn rd(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn rd_at(dir: &str, attr: &str) -> Option<String> {
    rd(format!("{}/{}", dir, attr))
}

/// 找 INOU0000:XX 设备目录
fn find_ec_dev() -> Option<String> {
    let entries = fs::read_dir("/sys/bus/platform/devices").ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("INOU0000:"))
        .map(|n| format!("/sys/bus/platform/devices/{}", n))
}

/// 按 hwmon 的 name 找目录
fn find_hwmon(want: &str) -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/hwmon").ok()?;
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if rd(p.join("name")).as_deref() == Some(want) {
            return Some(p);
        }
    }
    None
}


/// 键盘背光 LED（名字里带 kbd），本机目前没有
fn find_led_kbd() -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/leds").ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.contains("kbd") && !n.contains("capslock") && !n.contains("numlock"))
        .map(|n| PathBuf::from("/sys/class/leds").join(n))
}

fn millideg_to_c(v: &str) -> String {
    match v.trim().parse::<i64>() {
        Ok(m) => format!("{:.1} °C", m as f64 / 1000.0),
        Err(_) => v.to_string(),
    }
}

fn first_bat() -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("BAT"))
        .map(|n| PathBuf::from("/sys/class/power_supply").join(n))
}

fn nvidia_gpu_line() -> String {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,power.draw,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "无数据".to_string()
            } else {
                s
            }
        }
        Ok(o) => format!("nvidia-smi 失败: {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(_) => "nvidia-smi 不可用".to_string(),
    }
}

// ---------------------------------------------------------------- 提权

/// 经 pkexec 调用白名单助手（polkit 规则对本机用户免密）。参数直接传 argv，不经 shell。
fn priv_helper(args: &[&str]) -> Result<String, String> {
    let out = Command::new("pkexec")
        .arg(HELPER)
        .args(args)
        .output()
        .map_err(|e| format!("无法调用 pkexec: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.is_empty() {
            Err(format!("提权失败或已取消（exit {:?}）", out.status.code()))
        } else {
            Err(err)
        }
    }
}

fn priv_write(path: &str, val: &str) -> Result<String, String> {
    priv_helper(&["write", path, val])
}

/// 切换性能档：功耗墙 + cTGP 的数值统一维护在 /usr/local/bin/ryzenadj-profile。
fn profile_cmd(key: &str) -> Result<String, String> {
    priv_helper(&["profile", key])
}

// ---------------------------------------------------------------- UI 小工具

fn labeled_row(label: &str, widget: &impl IsA<gtk4::Widget>) -> gtk4::Box {
    let b = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    b.set_margin_start(12);
    b.set_margin_end(12);
    let l = gtk4::Label::new(Some(label));
    l.set_width_chars(14);
    l.set_xalign(0.0);
    b.append(&l);
    widget.set_hexpand(true);
    b.append(widget);
    b
}

fn section(title: &str) -> (gtk4::Box, gtk4::Box) {
    let frame = gtk4::Frame::new(Some(title));
    frame.set_margin_start(12);
    frame.set_margin_end(12);
    frame.set_margin_top(6);
    frame.set_margin_bottom(6);
    let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    inner.set_margin_top(10);
    inner.set_margin_bottom(10);
    frame.set_child(Some(&inner));
    // 返回 (供上层直接 append 的容器, 用于往里塞内容的 inner)
    let wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    wrap.append(&frame);
    (wrap, inner)
}

fn bool_attr(dev: &str, attr: &str) -> Option<bool> {
    rd_at(dev, attr).map(|v| v == "1")
}

fn parse_list(s: &str) -> Vec<f64> {
    s.split_whitespace().filter_map(|x| x.parse::<f64>().ok()).collect()
}

/// 一组 LED 的 RGB + 亮度控制。键盘背光和机身灯条共用。
///
/// 注意：颜色通道上限来自 `multi_max_intensity`，亮度上限来自 `max_brightness`，
/// 两者常常不相等（例如键盘背光是 50 / 4），不能混用同一个值。
fn build_led_controls(ctx: &Rc<Ctx>, dev: &str, lp: &Path, parent: &gtk4::Box, animation: bool) {
    let imax_list = rd(lp.join("multi_max_intensity")).map(|s| parse_list(&s)).unwrap_or_default();
    let cur_list = rd(lp.join("multi_intensity")).map(|s| parse_list(&s)).unwrap_or_default();
    let fallback = rd(lp.join("max_brightness"))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(255.0);
    let bmax = rd(lp.join("max_brightness"))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(fallback);

    let channels = [("红", 0), ("绿", 1), ("蓝", 2)];
    let mut scales = Vec::new();
    for (name, i) in channels {
        let imax = imax_list.get(i).copied().unwrap_or(fallback);
        let sc = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, imax, 1.0);
        sc.set_digits(0);
        sc.set_value(cur_list.get(i).copied().unwrap_or(0.0));
        parent.append(&labeled_row(name, &sc));
        scales.push(sc);
    }

    let bri = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, bmax, 1.0);
    bri.set_digits(0);
    bri.set_value(
        rd(lp.join("brightness")).and_then(|v| v.parse::<f64>().ok()).unwrap_or(bmax),
    );
    parent.append(&labeled_row("亮度", &bri));

    let btn = gtk4::Button::with_label("应用颜色与亮度");
    btn.set_margin_start(12);
    btn.set_margin_end(12);
    parent.append(&btn);
    {
        let lp = lp.to_path_buf();
        let s = ctx.status.clone();
        let sc = scales.clone();
        let b = bri.clone();
        btn.connect_clicked(move |_| {
            let val = sc.iter().map(|x| (x.value() as u32).to_string()).collect::<Vec<_>>().join(" ");
            let bri = b.value() as u32;
            let r = priv_write(&lp.join("multi_intensity").to_string_lossy(), &val)
                .and_then(|_| priv_write(&lp.join("brightness").to_string_lossy(), &bri.to_string()));
            match r {
                Ok(_) => s.set_text(&format!("颜色 {}；亮度 {}", val, bri)),
                Err(e) => s.set_text(&format!("写入失败：{}", e)),
            }
        });
    }

    if !animation {
        return;
    }
    // 两个动画开关
    for (attr, label) in [("rainbow_animation", "彩虹动画"), ("breathing_in_suspend", "待机呼吸灯")] {
        if let Some(v) = bool_attr(dev, attr) {
            let sw = gtk4::Switch::new();
            sw.set_active(v);
            sw.set_halign(gtk4::Align::Start);
            parent.append(&labeled_row(label, &sw));
            let updating = Rc::new(Cell::new(false));
            let dev2 = dev.to_string();
            let s = ctx.status.clone();
            let u = updating.clone();
            sw.connect_active_notify(move |w| {
                if u.get() {
                    return;
                }
                let val = if w.is_active() { "1" } else { "0" };
                match priv_write(&format!("{}/{}", dev2, attr), val) {
                    Ok(_) => s.set_text(&format!("{} 已设为 {}", attr, val)),
                    Err(e) => {
                        s.set_text(&format!("{} 写入失败：{}", attr, e));
                        u.set(true);
                        w.set_active(!w.is_active());
                        u.set(false);
                    }
                }
            });
        }
    }
}

// ---------------------------------------------------------------- 主界面

struct Ctx {
    dev: String, // INOU0000:XX 路径
    status: gtk4::Label,
}

impl Ctx {
    fn attr(&self, a: &str) -> Option<String> {
        rd_at(&self.dev, a)
    }
}

fn build_ui(app: &Application) {
    let dev = match find_ec_dev() {
        Some(d) => d,
        None => {
            let w = ApplicationWindow::builder().application(app).title("蛟龙控制中心").build();
            let l = gtk4::Label::new(Some(
                "找不到 INOU0000:XX 设备。\n请先加载驱动：\n  sudo modprobe uniwill-next force=1",
            ));
            w.set_child(Some(&l));
            w.present();
            return;
        }
    };
    let kbd_led = find_led_kbd();
    let hwmon = find_hwmon("uniwill");

    let window = ApplicationWindow::builder()
        .application(app)
        .title("蛟龙 16K 控制中心")
        .default_width(560)
        .default_height(680)
        .build();

    let notebook = gtk4::Notebook::new();

    // ---- 状态栏 ----
    let status = gtk4::Label::new(Some("就绪"));
    status.set_xalign(0.0);
    status.set_margin_start(12);
    status.set_margin_bottom(8);
    status.set_wrap(true);
    status.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    status.set_max_width_chars(40); // 长消息折行，不撑开窗口
    status.set_selectable(true);
    status.add_css_class("dim-label");

    let ctx = Rc::new(Ctx {
        dev: dev.clone(),
        status: status.clone(),
    });

    // ============================ 页 1：监控 ============================
    let (mon_wrap, mon) = section("实时监控（每 2 秒刷新）");
    let m_cpu = gtk4::Label::new(Some("—"));
    let m_gpu = gtk4::Label::new(Some("—"));
    let m_fan1 = gtk4::Label::new(Some("—"));
    let m_fan2 = gtk4::Label::new(Some("—"));
    let m_bat = gtk4::Label::new(Some("—"));
    let m_nv = gtk4::Label::new(Some("—"));
    let m_ctgp = gtk4::Label::new(Some("—"));
    let m_drv = gtk4::Label::new(Some("—"));
    for (name, w) in [
        ("CPU 温度", &m_cpu),
        ("GPU 温度 (EC)", &m_gpu),
        ("风扇 1 (Main)", &m_fan1),
        ("风扇 2 (Secondary)", &m_fan2),
        ("电池", &m_bat),
        ("NVIDIA", &m_nv),
        ("cTGP 偏移", &m_ctgp),
        ("驱动", &m_drv),
    ] {
        w.set_xalign(0.0);
        w.set_selectable(true);
        mon.append(&labeled_row(name, w));
    }

    let btn_refresh = gtk4::Button::with_label("立即刷新");
    btn_refresh.set_margin_start(12);
    btn_refresh.set_margin_end(12);
    mon.append(&btn_refresh);

    notebook.append_page(&mon_wrap, Some(&gtk4::Label::new(Some("监控"))));

    // ============================ 页 2：性能 ============================
    let (perf_wrap, perf) = section("性能档位（ryzenadj 功耗墙 + cTGP）");
    let hint = gtk4::Label::new(Some(
        "本机没有 platform_profile，性能档由 ryzenadj 设定功耗墙 + EC 的 cTGP 偏移实现。\n功耗墙重启后失效，可用 systemd 服务开机套用。",
    ));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.set_margin_start(12);
    hint.add_css_class("dim-label");
    perf.append(&hint);

    let profiles = [("low-power", "静音 / 长续航"), ("balanced", "均衡"), ("performance", "性能")];
    for (key, label) in profiles {
        let b = gtk4::Button::with_label(label);
        b.set_margin_start(12);
        b.set_margin_end(12);
        let s = ctx.status.clone();
        let k = key.to_string();
        b.connect_clicked(move |_| {
            match profile_cmd(&k) {
                Ok(out) => s.set_text(&format!("已切到 {}：{}", k, out.replace('\n', " "))),
                Err(e) => s.set_text(&format!("切换 {} 失败：{}", k, e)),
            }
        });
        perf.append(&b);
    }

    let (ctgp_wrap, ctgp_box) = section("cTGP 偏移（GPU 功耗）");
    let ctgp_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 60.0, 1.0);
    ctgp_scale.set_digits(0);
    ctgp_scale.set_value(ctx.attr("ctgp_offset").and_then(|v| v.parse().ok()).unwrap_or(0.0));
    ctgp_scale.add_mark(0.0, gtk4::PositionType::Bottom, Some("基准"));
    ctgp_box.append(&labeled_row("偏移 (W)", &ctgp_scale));
    let ctgp_note = gtk4::Label::new(Some(
        "建议 ≤ 20。拉满会挤掉 Dynamic Boost 的全部窗口（驱动上限 255，EC 会自行钳制）。",
    ));
    ctgp_note.set_xalign(0.0);
    ctgp_note.set_wrap(true);
    ctgp_note.set_margin_start(12);
    ctgp_note.add_css_class("dim-label");
    ctgp_box.append(&ctgp_note);
    let btn_ctgp = gtk4::Button::with_label("应用 cTGP");
    btn_ctgp.set_margin_start(12);
    btn_ctgp.set_margin_end(12);
    ctgp_box.append(&btn_ctgp);

    {
        let dev = dev.clone();
        let scale = ctgp_scale.clone();
        let s = ctx.status.clone();
        let m = m_ctgp.clone();
        btn_ctgp.connect_clicked(move |_| {
            let v = scale.value() as i32;
            match priv_write(&format!("{}/ctgp_offset", dev), &v.to_string()) {
                Ok(_) => {
                    s.set_text(&format!("cTGP 偏移已设为 +{} W", v));
                    if let Some(cur) = rd_at(&dev, "ctgp_offset") {
                        m.set_text(&format!("{} W (基准之上)", cur));
                    }
                }
                Err(e) => s.set_text(&format!("写入失败：{}", e)),
            }
        });
    }

    let perf_all = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    perf_all.append(&perf_wrap);
    perf_all.append(&ctgp_wrap);
    notebook.append_page(&perf_all, Some(&gtk4::Label::new(Some("性能"))));

    // ============================ 页：自定义 ============================
    let (cu_wrap, cu) = section("自定义模式（CPU 功耗墙 / 温度墙 / 睿频 / cTGP）");
    let conf = fs::read_to_string("/etc/jiaolong/custom.conf").unwrap_or_default();
    let conf_val = |k: &str, d: f64| -> f64 {
        conf.lines()
            .find_map(|l| l.strip_prefix(&format!("{}=", k)))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(d)
    };
    let mut cu_scales: Vec<gtk4::Scale> = Vec::new();
    for (name, key, lo, hi, def) in [
        ("持续功耗 STAPM (W)", "STAPM", 15.0, 90.0, 80.0),
        ("短时功耗 FAST (W)", "FAST", 15.0, 120.0, 100.0),
        ("中时功耗 SLOW (W)", "SLOW", 15.0, 90.0, 80.0),
        ("CPU 温度墙 (℃)", "TCTL", 60.0, 100.0, 99.0),
        ("GPU cTGP 偏移 (W)", "CTGP", 0.0, 60.0, 15.0),
    ] {
        let sc = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, lo, hi, 1.0);
        sc.set_digits(0);
        sc.set_value(conf_val(key, def));
        sc.set_hexpand(true);
        cu.append(&labeled_row(name, &sc));
        cu_scales.push(sc);
    }
    let boost_sw = gtk4::Switch::new();
    boost_sw.set_active(conf_val("BOOST", 1.0) != 0.0);
    boost_sw.set_halign(gtk4::Align::Start);
    cu.append(&labeled_row("CPU 睿频", &boost_sw));
    let epp_items = ["performance", "balance_performance", "balance_power", "power", "default"];
    let epp_combo = gtk4::ComboBoxText::new();
    for e in epp_items {
        epp_combo.append_text(e);
    }
    let cur_epp = conf
        .lines()
        .find_map(|l| l.strip_prefix("EPP="))
        .unwrap_or("balance_performance")
        .trim()
        .to_string();
    epp_combo.set_active(Some(epp_items.iter().position(|e| *e == cur_epp).unwrap_or(1) as u32));
    cu.append(&labeled_row("EPP 倾向", &epp_combo));
    let cu_note = gtk4::Label::new(Some(
        "风扇转速不可自定义：本机内核驱动没有风扇写入接口（pwm 只读），曲线由 EC 固件\n\
         按功耗档整表管理；功耗墙越高，EC 的曲线越激进。GPU 功耗墙在本机不被 NVIDIA 驱动\n\
         支持，只能靠 cTGP 偏移。FAST 不得低于 STAPM / SLOW。重启后由开机服务恢复，\n\
         要开机默认用自定义，把服务里的 PRESET 改成 custom。",
    ));
    cu_note.set_xalign(0.0);
    cu_note.set_wrap(true);
    cu_note.set_max_width_chars(60);
    cu_note.set_margin_start(12);
    cu_note.add_css_class("dim-label");
    cu.append(&cu_note);
    let cu_btn = gtk4::Button::with_label("应用并保存自定义");
    cu_btn.set_margin_start(12);
    cu_btn.set_margin_end(12);
    cu.append(&cu_btn);
    {
        let s = ctx.status.clone();
        let sc = cu_scales.clone();
        let bs = boost_sw.clone();
        let ec = epp_combo.clone();
        cu_btn.connect_clicked(move |_| {
            let mut args: Vec<String> = vec!["custom".into()];
            args.extend(sc.iter().map(|x| (x.value() as u32).to_string()));
            args.push(if bs.is_active() { "1" } else { "0" }.into());
            args.push(ec.active_text().map(|t| t.to_string()).unwrap_or_else(|| "balance_performance".into()));
            let refs: Vec<&str> = args.iter().map(|x| x.as_str()).collect();
            match priv_helper(&refs) {
                Ok(out) => s.set_text(&format!("自定义已应用：{}", out.replace('\n', " "))),
                Err(e) => s.set_text(&format!("自定义失败：{}", e)),
            }
        });
    }
    let cu_scroll = gtk4::ScrolledWindow::new();
    cu_scroll.set_child(Some(&cu_wrap));
    notebook.append_page(&cu_scroll, Some(&gtk4::Label::new(Some("自定义"))));

    // ============================ 页 3：键盘 RGB ============================
    let (kbd_wrap, kbd_page) = section("键盘背光 RGB（uniwill:multicolor:kbd_backlight）");
    if let Some(lp) = kbd_led.clone() {
        build_led_controls(&ctx, &dev, &lp, &kbd_page, false);
        let tip = gtk4::Label::new(Some(
            "注：硬件不支持纯黑，RGB 全 0 会被驱动抬成极暗白；要关灯把亮度拉到 0。",
        ));
        tip.set_xalign(0.0);
        tip.set_wrap(true);
        tip.set_margin_start(12);
        tip.add_css_class("dim-label");
        kbd_page.append(&tip);
    } else {
        let l = gtk4::Label::new(Some(
            "未发现键盘背光接口。\n\
             内核 7.2.x 自带的 uniwill-laptop 没有键盘背光代码，需要 uniwill-next（DKMS）；\n\
             内核 7.3 起主线已内置。可用 `dkms status` 确认模块是否已编译加载。\n\
             在那之前，键盘背光仍可用 Fn 组合键切换（由 EC 直接处理）。",
        ));
        l.set_xalign(0.0);
        l.set_wrap(true);
        l.set_margin_start(12);
        kbd_page.append(&l);
    }
    notebook.append_page(&kbd_wrap, Some(&gtk4::Label::new(Some("键盘 RGB"))));

    // 本机没有机身灯条：force=1 会虚报 uniwill:multicolor:status，故不提供灯条页。

    // ============================ 页 4：键盘 / 充电 ============================
    let (kb_wrap, kb) = section("键盘与触控板");
    for (attr, label) in [
        ("fn_lock", "FN 锁定"),
        ("super_key_enable", "Super(Win) 键"),
        ("touchpad_toggle_enable", "触控板开关快捷键"),
    ] {
        let sw = gtk4::Switch::new();
        sw.set_active(bool_attr(&dev, attr).unwrap_or(false));
        sw.set_halign(gtk4::Align::Start);
        kb.append(&labeled_row(label, &sw));
        let updating = Rc::new(Cell::new(false));
        let dev2 = dev.clone();
        let s = ctx.status.clone();
        let u = updating.clone();
        sw.connect_active_notify(move |w| {
            if u.get() {
                return;
            }
            let val = if w.is_active() { "1" } else { "0" };
            match priv_write(&format!("{}/{}", dev2, attr), val) {
                Ok(_) => s.set_text(&format!("{} 已设为 {}", label, val)),
                Err(e) => {
                    s.set_text(&format!("{} 写入失败：{}", label, e));
                    u.set(true);
                    w.set_active(!w.is_active());
                    u.set(false);
                }
            }
        });
    }

    let (chg_wrap, chg) = section("电池与供电");
    if let Some(bat) = first_bat() {
        let types = rd(bat.join("charge_types")).unwrap_or_default();
        if !types.is_empty() {
            let combo = gtk4::ComboBoxText::new();
            let mut active = 0u32;
            // 内核格式："Trickle [Standard] Long_Life"，方括号内是当前档位
            for (i, t) in types.split_whitespace().enumerate() {
                combo.append_text(t.trim_matches(|c| c == '[' || c == ']'));
                if t.starts_with('[') {
                    active = i as u32;
                }
            }
            combo.set_active(Some(active));
            chg.append(&labeled_row("充电档位", &combo));
            let bat2 = bat.clone();
            let s = ctx.status.clone();
            combo.connect_changed(move |c| {
                if let Some(v) = c.active_text() {
                    match priv_write(&bat2.join("charge_types").to_string_lossy(), v.as_str()) {
                        Ok(_) => s.set_text(&format!("充电档位 → {}", v)),
                        Err(e) => s.set_text(&format!("写入失败：{}", e)),
                    }
                }
            });
        }
    }
    if let Some(cur) = ctx.attr("usb_c_power_priority") {
        let combo = gtk4::ComboBoxText::new();
        let mut active = 0u32;
        for (i, t) in ["charging", "performance"].iter().enumerate() {
            combo.append_text(t);
            if cur.contains(&format!("[{}]", t)) {
                active = i as u32;
            }
        }
        combo.set_active(Some(active));
        chg.append(&labeled_row("USB-C 供电优先级", &combo));
        let dev2 = dev.clone();
        let s = ctx.status.clone();
        combo.connect_changed(move |c| {
            if let Some(v) = c.active_text() {
                match priv_write(&format!("{}/usb_c_power_priority", dev2), v.as_str()) {
                    Ok(_) => s.set_text(&format!("USB-C 优先级 → {}", v)),
                    Err(e) => s.set_text(&format!("写入失败：{}", e)),
                }
            }
        });
    }

    let page4 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    page4.append(&kb_wrap);
    page4.append(&chg_wrap);
    notebook.append_page(&page4, Some(&gtk4::Label::new(Some("键盘 / 充电"))));

    // ============================ 组装 ============================
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    root.append(&notebook);
    // 状态栏固定高度，消息再长也只在内部滚动，窗口尺寸不变
    let status_box = gtk4::ScrolledWindow::new();
    status_box.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    status_box.set_min_content_height(64);
    status_box.set_max_content_height(64);
    status_box.set_child(Some(&status));
    root.append(&status_box);
    window.set_child(Some(&root));

    // ---- 刷新逻辑 ----
    let refresh = {
        let dev = dev.clone();
        let hwmon = hwmon.clone();
        move || {
            if let Some(h) = &hwmon {
                if let Some(v) = rd(h.join("temp1_input")) {
                    m_cpu.set_text(&millideg_to_c(&v));
                }
                if let Some(v) = rd(h.join("temp2_input")) {
                    m_gpu.set_text(&millideg_to_c(&v));
                }
                if let Some(v) = rd(h.join("fan1_input")) {
                    m_fan1.set_text(&format!("{} RPM", v));
                }
                if let Some(v) = rd(h.join("fan2_input")) {
                    m_fan2.set_text(&format!("{} RPM", v));
                }
            } else {
                m_cpu.set_text("无 uniwill hwmon");
            }
            if let Some(b) = first_bat() {
                let cap = rd(b.join("capacity")).unwrap_or("?".into());
                let st = rd(b.join("status")).unwrap_or("?".into());
                let ct = rd(b.join("charge_types"))
                    .and_then(|t| {
                        t.split_whitespace()
                            .find(|x| x.starts_with('['))
                            .map(|x| x.trim_matches(|c| c == '[' || c == ']').to_string())
                    })
                    .unwrap_or_else(|| "默认".into());
                m_bat.set_text(&format!("{}%  {}  档位 {}", cap, st, ct));
            }
            if let Some(v) = rd_at(&dev, "ctgp_offset") {
                m_ctgp.set_text(&format!("{} W（基准之上）", v));
            }
            m_nv.set_text(&nvidia_gpu_line());
            let drv = fs::read_link(format!("{}/driver", dev))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "?".into());
            m_drv.set_text(&format!(
                "{} @ {}",
                drv,
                dev.rsplit('/').next().unwrap_or("?")
            ));
        }
    };

    refresh();
    {
        let r = refresh.clone();
        btn_refresh.connect_clicked(move |_| r());
    }
    let tick = refresh.clone();
    glib::timeout_add_local(Duration::from_millis(2000), move || {
        tick();
        glib::ControlFlow::Continue
    });

    window.present();
}

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}
