# jiaolong-ctl — 蛟龙 16K (MECHREVO GM6BG0Q) Linux 控制中心

给 **机械革命 蛟龙 16K（同方 TongFang GM6BG0Q 模具，Uniwill EC）** 在 Linux 下补齐原厂控制台的功能：

- 性能档位：静音 / 均衡 / 性能，**同时写 EC 档位寄存器**，所以风扇策略、功耗墙和**性能键上方的指示灯**都会跟着变
- 机身**性能键**（电源键旁）可用：按一下循环切档，并弹桌面通知
- **自定义模式**：CPU 功耗墙（STAPM/FAST/SLOW）、温度墙、睿频、EPP、GPU cTGP
- 实时监控：CPU/GPU 温度、双风扇转速、电池、NVIDIA 状态
- 键盘 RGB 背光、FN 锁定、Win 键、触控板快捷键、充电档位、USB-C 供电优先级

界面是 Rust + GTK4，所有特权操作经过一个**白名单助手**，不会以 root 运行整个 GUI。

> ⚠️ **只适用于蛟龙 16K（board_name = `GM6BG0Q`）。** 其他 Uniwill/同方机型可能有类似的 EC，但寄存器含义没有验证过，
> 脚本在非 GM6BG0Q 上会拒绝写 EC。使用风险自负，见文末「安全说明」。

## 快速开始（Arch Linux）

```bash
git clone https://github.com/dydydd/jiaolong-ctl.git
cd jiaolong-ctl
./install.sh
```

安装脚本会：装依赖（`ryzenadj` `acpi_call` `libnotify` `gtk4` `polkit` `rust`）→ 编译 → 安装助手、polkit 规则、
systemd 服务 → 设置 `acpi_call` 开机加载 → 启用开机套用档位和性能键监听。完成后运行 `jiaolong-ctl`，或在应用菜单搜「蛟龙」。

卸载：`./install.sh --uninstall`

### 前置：uniwill 驱动

本机的 EC 由内核 `uniwill-laptop` 驱动接管，但**主线驱动的 DMI 白名单里还没有 MECHREVO**，需要 `force=1`：

```bash
echo 'options uniwill-laptop force=1' | sudo tee /etc/modprobe.d/uniwill.conf
echo 'uniwill-laptop'                 | sudo tee /etc/modules-load.d/uniwill.conf
sudo modprobe uniwill-laptop force=1
ls /sys/bus/platform/devices/ | grep INOU        # 应出现 INOU0000:00
```

- 键盘背光（`uniwill:multicolor:kbd_backlight`）在内核 **7.3** 才并入主线；7.2.x 需要 DKMS 版 `uniwill-next`。
- `force=1` 会把所有功能都假定为存在，因此会冒出本机没有的节点（比如 `uniwill:multicolor:status` 灯条），GUI 已不提供灯条页。
- 内核补丁 `kernel-patch/0001-*.patch` 给主线驱动加了本机的 DMI 条目和明确的功能集（**尚未提交**），合入后就不再需要 `force=1`。
- Secure Boot 开着时 DKMS 模块无签名会加载失败。

## 使用

### 图形界面

| 页 | 内容 |
|---|---|
| 监控 | 温度、风扇、电池、NVIDIA、cTGP，每 2 秒刷新 |
| 性能 | 静音 / 均衡 / 性能 三个按钮 + cTGP 滑块 |
| 自定义 | STAPM / FAST / SLOW 功耗墙、温度墙、睿频、EPP、cTGP；点「应用并保存」 |
| 键盘 RGB | 键盘背光颜色和亮度 |
| 键盘 / 充电 | FN 锁定、Win 键、触控板开关、充电档位、USB-C 优先级 |

写操作经 `pkexec` 调用助手，由 polkit 规则对**本机活动会话里的当前用户免密放行**，其它 `pkexec` 用法仍然要密码。

### 三档预设（原厂值）

| 档位 | EC 寄存器 `0x0751` | CPU 功耗 (STAPM/SLOW) | FAST | Tctl | cTGP 偏移 |
|---|---|---|---|---|---|
| low-power 静音 | `0xA0` | 35 W | 45 W | 90 ℃ | 0 |
| balanced 均衡 | `0x00` | 65 W | 100 W | 95 ℃ | 5 |
| performance 性能 | `0x10` | 80 W | 100 W | 99 ℃ | 15 |

数值来自在 Windows 下对原厂控制台的 EC 实测（见 `docs/arch-manual-zh.md`），集中维护在 `system/ryzenadj-profile`。

### 命令行

```bash
sudo ryzenadj-profile performance      # low-power | balanced | performance | custom
cat /run/jiaolong-profile              # 当前档位
sudo ryzenadj -i                       # 读 SMU 实际功耗墙，核对是否生效
```

### 性能键

电源键旁的性能键（Uniwill WMI 事件 `0xB0`）由 `jiaolong-hotkeyd.service` 监听，每按一次循环：

静音 → 均衡 → 性能 →（保存过自定义参数时）自定义 → 静音……

```bash
journalctl -u jiaolong-hotkeyd -f      # 看按键日志
```

### 开机默认档位

默认开机套用 `performance`（与出厂开机状态一致）。想改，编辑 `/etc/systemd/system/ryzenadj-profile.service` 里的 `PRESET=`
（`low-power` / `balanced` / `performance` / `custom`），然后 `sudo systemctl daemon-reload`。
**功耗墙重启即失效**，所以必须靠这个服务重新套用。

## 原理

```
 性能键 → Uniwill WMI 事件 0xB0 → KEY_F14 → jiaolong-hotkeyd ┐
 GUI 按钮 → pkexec jiaolong-helper（白名单校验）──────────────┤
 开机 → ryzenadj-profile.service ─────────────────────────────┤
                                                              ▼
                                                   ryzenadj-profile <档位>
                          ┌───────────────────────────┼─────────────────────────┐
                          ▼                           ▼                         ▼
         EC 0x0751 ← acpi_call               ryzenadj 写 SMU            ctgp_offset (sysfs)
         (EC 自己切 SPL/风扇/指示灯)          (STAPM/FAST/SLOW/Tctl)      睿频 / EPP (sysfs)
```

- **EC 档位寄存器**：`0x0751` 的 `0xB0` 位。约定来自 TUXEDO 的 `tuxedo-drivers`（同款 Uniwill EC），
  并在本机实测：`0xA0 → SPL 35W`、`0x00 → 65W`、`0x10 → 80W`，与原厂三档完全一致。
- 通道是固件里 `INOU` 设备自带的 `ECRR`/`ECRW` 方法，经 `acpi_call` 调用；只在 `board_name == GM6BG0Q` 时写，
  先读后写、只改 `0xB0` 位、写后读回校验，任何异常放弃。
- 内核没有提供 `platform_profile`，所以用户态自己实现。

## 做不到的事（实测结论）

- **风扇转速不能自定义**：`hwmon` 的 `pwm1/pwm2` 只读，内核驱动没有风扇写入接口。风扇曲线由 EC 随档位管理，切档位会改变策略。
  EC 里有可写的曲线表（`0x0F00-0x0F5F`），但目前读出来全是 0、写入协议没有验证过，**没有实现**。
- **GPU 功耗墙不能调**：`nvidia-smi -pl` 在本机被驱动拒绝（"not supported"），GPU 只能靠 EC 的 `ctgp_offset`。
- **充电上限**：`force` 模式下驱动不提供 `charge_control_end_threshold`，只有 `charge_types`（Standard / Trickle / Long_Life）。
- **机身灯条**：本机没有。

## 目录

```
src/main.rs                 GTK4 界面
system/jiaolong-helper      特权白名单助手（root，sysfs 属性 + 档位 + 自定义参数）
system/ryzenadj-profile     切档脚本：EC 档位 + ryzenadj + cTGP + 睿频/EPP
system/jiaolong-hotkeyd     性能键监听服务
system/50-jiaolong.rules    polkit 规则（只放行助手）
system/*.service            systemd 单元
kernel-patch/               给主线 uniwill-laptop 加本机 DMI 条目的补丁（未提交）
docs/arch-manual-zh.md      逆向与 EC 实测记录、Arch 安装手册（部分命令已过时，如 mctl）
install.sh                  安装 / 卸载
```

## 安全说明

- EC 写入只有两处：`0x0751` 的档位位，以及 uniwill 驱动自己暴露的 sysfs 属性。**不要手工往别的 EC 寄存器写**，写错风扇/功耗相关寄存器可能导致过热或硬件损伤。
- `acpi_call` 允许 root 调用任意 ACPI 方法（`/proc/acpi/call` 仅 root 可访问）。介意的话可以只在需要时 `modprobe`，
  代价是开机档位和性能键在没加载时不会写 EC（脚本会提示并继续写 CPU/GPU 部分）。
- 助手用白名单校验路径与数值范围，polkit 规则只放行这一个程序、一个用户。

## 已知问题

- 性能键上方的指示灯是否随档位变色**尚未经作者确认**：EC 档位寄存器的写入已验证（SPL 随之变化），灯的表现请在 Issues 里反馈。
- 非 Arch 发行版需要自己装依赖；`install.sh` 只处理 `pacman`。
- 作者的机器 BIOS 为 `N.1.19MRO15`，其他 BIOS 版本未验证。

## 参考

- [tuxedo-drivers](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers) — Uniwill EC 寄存器约定
- 内核 `drivers/platform/x86/uniwill/` — uniwill-laptop 驱动
- [RyzenAdj](https://github.com/FlyGoat/RyzenAdj)

## 许可

MIT
