# 蛟龙16K (MECHREVO GM6BG0Q) → Arch Linux 完整手册

> 汇总日期：2026-09-29。本文档合并了三部分工作成果，全部结论**在本机 Windows 侧实测验证**：
>
> 1. **原厂控制台逆向**（静态分析 + 配置文件，未执行原厂程序、未写 EC）
> 2. **EC RAM 实测**（管理员权限 WMI 直读 + 模式切换三快照 diff，全程只读）
> 3. **Arch 落地方案**（驱动、功能复现、踩坑清单）
>
> 配套文件：`mctl`（命令行控制中心，Python 单文件零依赖）、`setup.sh`（半自动部署）、`逆向报告.md`（逆向原始记录）、`admin/`（实测脚本与快照数据）。
> **本手册自包含**：逆向原始记录与全部实测数据已收录进正文，只带这一个文件也能完成迁移；配套文件只是锦上添花。遇到手册没覆盖的情况，直接跳到第 17 节。

---

# 第一部分 · 结论速览

## 一页纸结论

**功能不是用不了，是入口换了。**

- 蛟龙16K = **同方 TongFang GM6BG0Q 模具**，EC 是标准 **Uniwill EC**（设备 ID `ACPI\INOU0000`），与主线内核 6.19+ 的 `uniwill-laptop` 驱动一字不差
- 但该驱动的 **DMI 白名单没有 MECHREVO**，必须 `force=1` 手动加载
- 原厂控制台的全部功能在 Linux 上的等价物：

| 控制台功能 | Linux 等价物 | 状态 |
|---|---|---|
| 性能模式（静音/均衡/狂暴） | `platform_profile` / `mctl profile`（进阶：ryzenadj） | ✅ 可复现 |
| 风扇控制 | EC 内置曲线，切模式即切换（曲线本体已实测） | ✅ 不用管 |
| cTGP 显卡功耗 | 内核 7.0+ `ctgp_offset` sysfs | ✅ 可复现 |
| 键盘 RGB / 灯条 | 内核 LED 子系统（`/sys/class/leds`） | ✅ 已接好 |
| 充电阈值 | `charge_control_end_threshold`（force 模式下 uniwill 不提供，需 DKMS 版） | ⚠️ 换途径 |
| GUI 控制台 | `tuxedo-control-center-bin`（TUXEDO 现成方案） | ✅ 可装 |

- EC 实测摸清了原厂控制台的每个动作：**当前 SPL 寄存器（0x046A/0x0783）、FPPT（0x0785）、风扇曲线表（0x0F00-0x0F5F）**，切模式 = SPL 翻转 + 曲线表整表重写，全部在固件侧完成
- 机器一直跑在 **80W 性能档**（OperatingMode=2），不是早先误判的 35W 办公档

## 0. 机器底细（已从 Windows 侧抓取，装完系统就不好查了）

| 项目 | 值 |
| --- | --- |
| 机型 | MECHREVO Jiaolong16K Series GM6BG0Q |
| 主板 | MECHREVO GM6BG0Q（同方模具，RTX 4060 版） |
| BIOS | American Megatrends N.1.19MRO15（2023-06-29） |
| CPU | AMD Ryzen 7 7735H（Rembrandt, Zen3+, 8C/16T） |
| 核显 | AMD Radeon 680M（Rembrandt） |
| 独显 | NVIDIA GeForce RTX 4060 Laptop GPU（140W 满血，代号 GN21） |
| 屏幕 | 16" 2560×1600 165Hz 100% sRGB，DC 调光 |

装完自己核对批次差异：

```bash
lspci -nn | grep -iE "ethernet|network|vga|3d"   # 网卡/显卡具体型号
cat /sys/class/dmi/id/board_name                  # 应为 GM6BG0Q
```

> **历史坑位提醒**：这台机器早期在 Linux 6.0~6.2 有内置键盘完全失灵的 bug。Hans de Goede 已把 `GM6BG0Q` 加进内核 `irq1_edge_low_force_override` 白名单并合入主线。**现代内核直接正常，不要加 `i8042.nomux` 之类的老参数。**

---

# 第二部分 · 原厂控制台逆向

## 1. 控制台架构

控制台装在 `C:\Program Files\OEM\机械革命电竞控制台`（GameViewer 是网易远控软件，与本机无关）。

```
机械革命电竞控制台 (UWP 界面, GamingCenter3_Cross.UWP)
        │
        ├── UniwillService\GCUBridge.exe      .NET 桥接服务（代码已混淆）
        │        ├── UEFI_Firmware.dll        读写 UEFI NVRAM 变量 UniWillVariable
        │        └── 通过 System.Management 调 WMI
        │
        ├── UniwillService\MyControlCenter\
        │        ├── GCUService.exe           主服务（12MB）
        │        ├── ACPIDriverDll.dll        用户态 EC 访问库（内部名 UWACPI）
        │        ├── NVControlSetting.dll     NVIDIA 设置
        │        ├── GPUInfoDLL.dll / IntelOverclockingSDK.dll
        │        ├── UserPofiles\             ★ 性能模式配置（本机实际值）
        │        ├── UserFanTables\           ★ 风扇曲线（按机型分目录）
        │        ├── KeyboardManager\         键盘灯效配置
        │        └── AppSettings\             应用/模式绑定
        │
        └── UWACPIDriver\UWACPIDriver.sys     内核驱动，绑定 ACPI\INOU0000
                符号链接: \DosDevices\ACPIDriver
```

**两条通道**：大部分设置经 EC（`ACPI\INOU0000`），少部分（RGB 灯效、充电限制等）存 UEFI NVRAM 变量。

**决定性证据（本机确认）**：

- INF 里写的硬件 ID 是 `ACPI\INOU0000` —— 和主线内核 `uniwill-laptop` 驱动的设备 ID **一字不差**
- 设备管理器确认存在：`ACPI\INOU0000\0`，显示名 `ACPIDriver`，状态 OK
- WMI 类 `AcpiTest_MULong` 在 `root\wmi` 里存在，正是 EC 访问接口
- 厂商字符串：`Uniwill Technology Inc.` / `TONGFANG HONGKONG LIMITE`

## 2. 性能模式参数（本机实际配置）

文件：`UniwillService\MyControlCenter\UserPofiles\Mode{1-4}_Profile1.json`

| 参数 | Mode1 | Mode2 | Mode3 | Mode4 |
|---|---|---|---|---|
| AMD SPL / PL1 | **65 W** | **35 W** | **80 W** | **65 W** |
| AMD SPPT / PL2 | 65 W | 35 W | 80 W | 65 W |
| AMD FPPT / PL4 | 100 W | 100 W | 100 W | 100 W |
| TCC 温度目标 | 99 °C | 99 °C | 99 °C | 99 °C |
| cTGP 开关 | 开 | 关 | 开 | 开 |
| cTGP 目标 | 115 W | 115 W | **140 W** | 115 W |
| DynamicBoost | 关 | 关 | 关 | **开**（25W） |
| GPU 核心/显存偏移 | 0 / 0 | 0 / 0 | **+100 / +500** | 0 / 0 |
| GPU 温度目标 | 87 °C | 87 °C | 87 °C | 87 °C |
| 风扇表 | M1T1 | M2T1 | M3T1 | M4T1 |

`MainOption.json` 的 **`OperatingMode`** 是当前档位索引。**实测纠正（第三部分 diff 实验）：`OperatingMode=2` 时 EC 里 SPL=80W（性能档），`=0` 时 SPL=35W（静音档）——索引不等于 `Mode{N+1}` 文件号**，早先"OperatingMode=2 → Mode2 35W"的推断是错的。

`MainOption.json` 还有 `GamingProfileIndex / OfficeProfileIndex / TurboProfileIndex / CustomProfileIndex` 四个字段，说明四模式对应 游戏/办公/狂暴/自定义。

## 3. 风扇曲线格式与关键发现

文件：`UserFanTables\<机型代号>\M{1,2}T{1,2,3}.json`，结构：

```json
{
  "PL1": "35", "PL2": "55", "TCC": "-5", "CTGP": "0", "DB": "25",
  "CpuTemp_DefaultMaxLevel": 11, "GpuTemp_DefaultMaxLevel": 11,
  "CPU": [ {"ID":0,"UpT":0,"DownT":52,"Duty":0},
           {"ID":1,"UpT":54,"DownT":56,"Duty":30}, ... ],
  "GPU": [ {"ID":0,"UpT":0,"DownT":41,"Duty":0}, ... ]
}
```

- CPU/GPU 各 16 级，每级 `UpT`（升温阈值）/`DownT`（降温阈值）/`Duty`（占空比）
- `UpT ≠ DownT` 是**滞回设计**，防温度抖动导致风扇反复变速
- 尾部未用级用 `255/255/255` 填充

**关键发现**：`UserFanTables` 下的机型代号全是 TUXEDO 的 PH4/PH6 系列（24 个），**没有 GM6BG0Q**；且蛟龙16K 引用的 `M3T1`/`M4T1` 文件根本不存在。

→ **结论：蛟龙16K 的风扇曲线由 EC/BIOS 内置管理，Windows 控制台只是切换性能模式，不直接下发曲线。**（第三部分 EC 实测已直接看到曲线本体并证实随模式重写）

这对 Linux 是好消息：不需要复刻风扇曲线，切对性能模式即可。

## 4. UEFI NVRAM 变量

变量名 `UniWillVariable`，GUID `9f33f85c-13ca-4fd1-9c4a-96217722c593`（从 `UEFI_Firmware.dll` 和 `GCUBridge.exe` 提取）。

控制台读写的字段（GCUBridge 日志格式串里出现）：

```
PowerMode            BatteryLimitation      ChargeMinimumLimit / ChargeMaximumLimit
RGBKeyboard08        RGBKeyboard1A          SmartLightbar08 / SmartLightbar1A
RGBLightbarMode      RGBLightbarMode_R / _G / _B
OemDisplayMode       KeyboardType           FnKeyStatus
```

后缀 `08`/`1A` 疑似 EC 寄存器地址，`_R/_G/_B` 是灯条三通道。

**Windows 侧读取结论**：即使提权到管理员，`AdjustTokenPrivileges` 返回 `ERROR_NOT_ALL_ASSIGNED`（1300）——这台机器令牌里根本没有 `SeSystemEnvironmentPrivilege`（被系统策略收走），`GetFirmwareEnvironmentVariable` 恒返回 err 1314。

**不影响实战**：Linux 侧 root 直接读文件，不依赖任何特权 API：

```
/sys/firmware/efi/efivars/UniWillVariable-9f33f85c-13ca-4fd1-9c4a-96217722c593
```

定位字段偏移的办法：改一个设置（比如充电上限 100%→80%），前后各 dump 一次做 diff。装完 Arch 第一天做也一样。

## 5. 静态逆向没拿到的东西

| 目标 | 结果 | 原因 |
|---|---|---|
| EC 寄存器地址表 | ❌ | GCUBridge 混淆，方法名变 `A.IDw`；地址运行时计算，明文无 `0xXXXX` |
| UEFI 变量实际值（Windows 侧） | ❌ | 令牌无 `SeSystemEnvironmentPrivilege`（见第 4 节），Linux 侧不受影响 |
| ACPI DSDT | ❌ | Windows 注册表里的表解析失败 |
| ~~WMI EC 实时读数~~ | ✅ | 管理员权限下拿到，见第三部分 |

---

# 第三部分 · EC RAM 实测（管理员 WMI + 模式切换 diff，全程只读）

地址表被混淆拿不到，但硬件自己不会混淆——用管理员权限通过 WMI 直读 EC RAM，把原厂控制台背后的真实硬件状态刷了出来。

## 6.1 访问通道（Windows 上可复现）

```powershell
# WMI 类：root\wmi:AcpiTest_MULong，本机 10 个实例（ACPI\PNP0C14\1_0 … 1_9）
# 读方法 GetSetULong，参数 Data 的编码：
#   Data = addr | (value << 16) | (op << 32)
#   读操作 op = 0x0100  →  base = 0x0100 << 32 = 1099511627776
#   返回值低 16 位 = 两个字节：低字节 = EC[addr]，次字节 = EC[addr+1]
#   返回 0xFEFEFEFE = EC 通信失败
$o = Get-CimInstance root\wmi:AcpiTest_MULong | Where-Object InstanceName -like "*1_0"
Invoke-CimMethod -InputObject $o -MethodName GetSetULong -Arguments @{ Data = 1099511627776 + 0x043E }
```

低 16 位含相邻两字节已交叉验证：`EC[0x043E]` 从 `0x043D` 的高字节和 `0x043E` 的低字节读到的值一致。

## 6.2 EC RAM 地址地图（实测汇总）

> ⚠️ 所有"疑似"判读基于采样特征与已知配置的量级吻合，**未经写验证**。全程只读，切勿照此裸写 EC。

**温度/传感器区 0x0400-0x04A0**

| 地址 | 观测值 | 判读 |
|---|---|---|
| `0x043E` | 77 → 55 → 49（动态） | **CPU 温度 °C** |
| `0x044F` | 47 → 38（动态） | **GPU 温度 °C**（0x072D 镜像同值） |
| `0x046A-0x046B` | 80↔35 随模式翻转 | **当前 CPU SPL（W）** ★ |
| `0x046F` | 64(100) | 疑似电池电量 % |
| `0x0436-0x0439` | F0 0A DB 3E | 未知（疑似电池相关） |
| `0x0461`/`0x0463`/`0x0466` | 60 / 99 / 64 | 疑似阈值/百分比 |

**风扇区 0x0700-0x077F**

| 地址 | 观测值 | 判读 |
|---|---|---|
| `0x0744` | 25↔0 随模式 | **CPU 风扇转速原始值**（×60 ≈ RPM；静音档停转时同步归 0，强证据） |
| `0x074C` | 1D 恒定 | 疑似 GPU 风扇转速（弱证据） |
| `0x0743` | 05↔01 随模式 | 疑似风扇策略 ID |
| `0x0746-0x0748` | 00 00 80 | tuxedo 驱动风扇 duty 写入地址族 |
| `0x0751` | 10↔A0 随模式 | 模式相关，语义不明 |
| `0x072D` | =GPU 温度 | GPU 温度镜像 |
| `0x0730-0x0737` | 65/65/100/1、35/35/100/1 | 三快照不变，排除是模式槽位（65/35 与 USB-C PD 供电档吻合，疑似功率契约） |
| `0x076F-0x077D` | 全 FF | 填充 |
| `0x077E-0x077F` | **55 AA** | 区域签名 |

**键盘背光/灯条区 0x0780-0x07BF**

| 地址 | 观测值 | 判读 |
|---|---|---|
| `0x0782-0x0785` | 1D / SPL / SPL / 100 | **(X, SPL, SPL, FPPT) 结构体**（非背光参数，SPL 随模式 80↔35） |
| `0x0783-0x0784` | 80↔35 | 当前 SPL 第二份拷贝 ★ |
| `0x0785` | 64(100) 恒定 | **FPPT = 100W**（四模式 JSON 都是 100，完美吻合）★ |
| `0x0788` | 25↔0 | 与 0x0744 同步（CPU 风扇） |
| `0x078C` | 68(104) | 与 UEFI 字段名 `RGBKeyboard08/1A` 地址族吻合 |
| `0x0794-0x079D` | 82 45 84 56 04 E3 … | 未知（成对出现，疑似灯效参数） |

**风扇曲线表 0x0F00-0x0F5F ★ 最大收获**

EC 内置曲线实时可见，结构与 JSON 格式完全对应（UpT/DownT 滞回 + Duty + 0xFF 填充）：

```
80W 档（OperatingMode=2）
CPU 风扇
  UpT   @0x0F00: 53 57 61 65 69 73 77 81 85 87 FF…   ← 10 级
  DownT @0x0F10: 00 48 50 60 64 68 72 76 80 84 86 FF… ← 滞后 3~5°C
  Duty  @0x0F20: 00 60 60 70 90 96 100 120 150 180 200… ← 封顶 200
GPU 风扇
  UpT   @0x0F30: 50 50 53 56 58 60 62 65 68 70 FF…    ← 10 级
  DownT @0x0F40: 48 48 52 55 57 59 60 64 67 69 FF…
  Duty  @0x0F50: 0 0 60 70 70 70 76 80 100 150 180 200… ← 封顶 200

35W 档（OperatingMode=0）
  CPU UpT 8 级 53~69°C，Duty 封顶 110；GPU UpT 8 级 49~62°C，Duty 封顶 90
```

要点：
- Duty 刻度是 **0-200**（不是 0-100），200 平台 = 满速
- **曲线表随性能模式整表重写、切回原模式完全复原**（6.4 节 diff 实测）
- 静音档 CPU 温度低于第一阈值 53°C 时风扇直接停转（0x0744=0 实测）

**功耗区 0x1800-0x1820**

| 地址 | 观测值 | 判读 |
|---|---|---|
| `0x1803` | 03 三快照不变 | **不是**模式寄存器 |
| `0x1804` | 100→60→60 动态 | 随负载变、与模式无关；疑似 CPU 当前功耗 |
| `0x1805`/`0x1808` | 96(150) 不变 | 疑似某功耗上限 W |
| `0x1809` | 80→60→60 动态 | 疑似 GPU 当前功耗（随负载不随模式） |
| `0x1800-0x1801` | E0 C8 | 未知 |

## 6.3 待做实验清单（只读 diff，随时可做）

1. ~~切性能模式 diff~~ → **已做，见 6.4**
2. ~~风扇转速验证~~ → 已部分验证（停转归 0）
3. **改灯效 diff**：控制台改键盘颜色/亮度，diff `0x0780-0x07BF`
4. **改充电上限 diff**：80% ↔ 100%，diff `0x0430-0x04A0` + UEFI 变量
5. **cTGP 切换 diff**：115W/140W 两档间切，看 EC 哪个字节动

⚠️ **所有实验保持只读**。绝不盲目写 EC 寄存器——写错风扇 duty=0 或功耗墙地址，轻则过热降频，重则硬件损伤。写操作必须走 Linux 内核驱动接口（有安全封装）。

## 6.4 模式切换 diff 实验 ★ 决定性证据

方法：快照脚本扫 `0x0000-0x001F / 0x0400-0x04BF / 0x0700-0x07BF / 0x0F00-0x0F5F / 0x1800-0x1820`，三份快照：A（OperatingMode=2）→ 控制台切档 → B（OperatingMode=0）→ 切回 → C（OperatingMode=2）。判据：**A==C≠B = 模式相关**（可逆性强信号）。

结果：A/B 差异 66 个寄存器，其中 **58 个完全可逆（A==C≠B）**，动态噪声 5 个，切回动作本身 2 个。

1. **EC 里就有"当前 SPL"寄存器，随模式 80↔35 翻转**：`0x046A/0x046B`（镜像 `0x0783/0x0784`）= 当前 SPL（W）；`0x0785` = FPPT 恒 100W
2. **风扇曲线表随模式整表重写、完全复原**（0x0F02-0x0F5F 共 49 字节全部可逆）
3. 模式相关但语义未明：`0x0743`（05↔01）、`0x0751`（10↔A0）、`0x0449`（0E↔0C）、`0x07A6`（+1 不回退，疑似计数器，存疑）
4. **推翻旧结论**：OperatingMode=2 是 80W 性能档、=0 是 35W 静音档——索引与 Mode{N+1} 文件号不一一对应

**对 Linux 的意义**：模式切换的全部硬件效果 = SPL 写入 + EC 曲线表重写，都在 EC/固件侧完成。内核 `uniwill-laptop` 驱动的 platform_profile 接口走同一套 WMI 通道，`mctl profile` 切档即可复现，**不需要**也不应该自己写这些寄存器。

---

# 第四部分 · Arch 安装与配置

## 7. 装机前：在 Windows 上先做这几件事

1. **关 BitLocker**（设备加密）——否则 Linux 读不了 Windows 分区，双系统还可能被锁。`设置 → 隐私和安全性 → 设备加密 → 关`
2. **关快速启动** —— `控制面板 → 电源选项 → 选择电源按钮的功能 → 取消"启用快速启动"`。不关会导致 Windows 关机后 NTFS 分区处于休眠状态，Linux 挂载失败或只读。
3. **进 BIOS 关 Secure Boot**（或装完后用 `sbctl` 自签，新手建议直接关）。DKMS 编译的模块没签名，Secure Boot 开着会加载失败——这是驱动"装了却不生效"的头号原因。
4. **记录 BIOS 里的显卡模式**（独显直连 / 混合）。同方模具的独显直连一般只能在 BIOS 里切，Linux 下热切换不可靠。
5. **确认 Windows 上性能控制台的档位名称和快捷键**，装完对比 Linux 下对不对得上。
6. 备好一个手机或第二台设备看这份文档。

## 8. 装 Arch：只说这台机器要注意的

安装流程照 [Arch Wiki Installation Guide](https://wiki.archlinux.org/title/Installation_guide) 走，以下是模具相关差异。

### 显卡驱动（Rembrandt 核显 + NVIDIA 独显，两个都要装）

```bash
sudo pacman -S mesa vulkan-radeon libva-mesa-driver   # AMD 核显
sudo pacman -S nvidia nvidia-utils nvidia-settings    # NVIDIA 独显
sudo pacman -S nvidia-prime                            # 提供 prime-run
```

默认走**按需渲染**（日常核显省电，需要时调独显）：

```bash
prime-run steam          # 用独显跑某个程序
prime-run glxinfo | grep OpenGL
__NV_PRIME_RENDER_OFFLOAD=1 __GLX_VENDOR_LIBRARY_NAME=nvidia <程序>   # 等价写法
```

### 桌面环境

KDE Plasma 或 GNOME 都行。**TUXEDO Control Center 的托盘图标在 GNOME 下需要 `AppIndicator and KStatusNotifierItem Support` 扩展**，KDE 原生支持，省心一点。

### 电源管理

```bash
sudo pacman -S power-profiles-daemon tlp
sudo systemctl enable --now power-profiles-daemon
```

## 9. 驱动：内核原生优先，但要手动 force

蛟龙16K 的 EC 设备 ID 是 `ACPI\INOU0000`。主线内核 6.19 起有原生驱动 **`uniwill-laptop`**（`CONFIG_UNIWILL_LAPTOP=m`）。Arch 2026.09.01 的 ISO 带 **Linux 7.2.2**，驱动肯定在。

**第一步：确认内核编译了这个模块**

```bash
zcat /proc/config.gz | grep -i UNIWILL
# 期望看到: CONFIG_X86_PLATFORM_DRIVERS_UNIWILL=y  和  CONFIG_UNIWILL_LAPTOP=m
```

**第二步：手动加载（DMI 白名单里没有 MECHREVO）**

```bash
sudo modprobe uniwill-laptop          # 先试正常加载
lsmod | grep uniwill

sudo modprobe uniwill-laptop force=1  # 没加载成功（大概率）就加 force
dmesg | tail -20
```

会看到两行**预期内**的警告：

```
uniwill: Loading on a potentially unsupported device
uniwill: Enabling potentially unsupported features
```

> **关于 force 的风险**：硬件证据很硬（原厂驱动就是 Uniwill 的 `UWACPIDriver.sys` + `ACPI\INOU0000`，白名单里也有 TUXEDO 的 GM6 系列主板），硬件本身兼容，风险主要是个别特性对不上。出问题 `sudo modprobe -r uniwill-laptop` 卸载即可，无持久影响。
> force 模式启用**除电池充电限制外**的全部特性（源码 `UINT_MAX & ~UNIWILL_FEATURE_BATTERY_CHARGE_LIMIT`），键盘背光按 4 档、灯条按 200 档初始化。

**第三步：让 force 永久生效**

```bash
echo 'options uniwill-laptop force=1' | sudo tee /etc/modprobe.d/uniwill.conf
echo 'uniwill-laptop' | sudo tee /etc/modules-load.d/uniwill.conf
```

重启后确认：

```bash
lsmod | grep uniwill
ls /sys/bus/platform/devices/ | grep -i INOU
```

`/sys/bus/platform/devices/` 下出现 `INOU0000:00` 就接管成功了。

**驱动提供的接口**（前缀 `/sys/bus/platform/devices/INOU0000:XX/`）：

| 属性 | 功能 | 内核版本 |
| --- | --- | --- |
| `fn_lock` | FN 锁定 0/1 | 6.19 |
| `super_key_enable` | Win 键开关 | 6.19 |
| `touchpad_toggle_enable` | 触控板开关 | 6.19 |
| `rainbow_animation` | 灯条彩虹动画 | 6.19 |
| `breathing_in_suspend` | 睡眠呼吸灯 | 6.19 |
| `ctgp_offset` | cTGP 显卡功耗偏移 | 7.0 |
| `usb_c_power_priority` | USB-C 供电策略 `charging`/`performance` | 7.1 |
| `ac_auto_boot` | 插电自动开机 | 7.1 |
| `usb_powershare_high` | 关机/休眠时 USB 对外供电 | 7.1 |

温度/风扇、电池、键盘背光走标准子系统（`/sys/class/hwmon`、`/sys/class/power_supply`、`/sys/class/leds`），`mctl detect` 会逐一列出。

**内核没接管时，才装 DKMS 驱动**

```bash
sudo pacman -S --needed base-devel linux-headers dkms
yay -S mechrevo-drivers-dkms
```

**图形控制台（可选）**

```bash
yay -S tuxedo-control-center-bin
sudo systemctl enable --now tccd.service
```

装完验证：

```bash
lsmod | grep -iE 'uniwill|tuxedo|clevo'
dkms status
dmesg | grep -iE 'uniwill|tuxedo'
```

驱动完全没加载时，优先怀疑 **Secure Boot 没关**。也可以直接 `sudo ./setup.sh` 一键跑完（GRUB 参数只给建议不代改）。

## 10. 四个功能的实际落点

### 10.1 性能模式

```bash
cat /sys/firmware/acpi/platform_profile_choices   # 有哪几档
cat /sys/firmware/acpi/platform_profile           # 当前档位
echo performance | sudo tee /sys/firmware/acpi/platform_profile
# 或
mctl profile get
sudo mctl profile set performance      # quiet | balanced | performance
```

#### 进阶：用 ryzenadj 精确复现原厂功耗墙

原厂四模式本质就是改 AMD 功耗墙（实测值见第二部分第 2 节：35W/65W/80W，FPPT 恒 100W，cTGP 115/140W）。`ryzenadj` 单位**毫瓦**：

```bash
yay -S ryzenadj
sudo ryzenadj -i                                   # 先读当前值存下来
# 均衡 65W 示例
sudo ryzenadj --stapm-limit=100000 --fast-limit=65000 --slow-limit=65000 --tctl-temp=99
```

> ⚠️ `AmdSPL/SPPT/FPPT` 与 `--stapm/--fast/--slow-limit` 的映射是**推测**，需实测校准：Windows 下切到某模式重启进 Linux，`sudo ryzenadj -i` 读实际值对比即知。EC 实测发现 SPL 值也写在 EC 里（0x046A），可对照验证。

> ⚠️ 已知坑：TCC 后台服务 `tccd` 在 `amd-pstate` 下疑似有 bug——反复把 CPU 调度器重置成 `performance`，**掉电飞快**就 `systemctl stop tccd` 试一下。

### 10.2 风扇

曲线由 EC 内置（实测见 6.2），切对性能模式即可，不用自己喂曲线。想看数据：

```bash
sensors          # lm_sensors
mctl fan         # 读 hwmon
mctl ecmap       # EC 地址地图（含曲线表实测值）
```

### 10.3 键盘 RGB 背光

```bash
ls /sys/class/leds/ | grep -iE 'kbd|keyboard'
```

- 认出 `*::kbd_backlight` → 亮度直接可控：`sudo mctl kbd brightness 2`
- 有 `multi_intensity` → 支持调色：`sudo mctl kbd color ff8800`
- 都没有 → 跑 `mctl detect` 把输出发回来，据此补

硬件快捷键（Fn 组合）通常不依赖驱动，先试快捷键。

### 10.4 电源与续航（充电阈值）

> ⚠️ 内核 `uniwill-laptop` 在 force 模式下**故意排除充电限制功能**。要充电阈值走通用接口或换 `mechrevo-drivers-dkms`。

```bash
# 标准接口（优先）
cat /sys/class/power_supply/BAT*/charge_control_end_threshold
echo 80 | sudo tee /sys/class/power_supply/BAT*/charge_control_end_threshold
sudo mctl battery limit 80

# 厂商接口（DKMS 驱动提供时）
cat /sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profiles_available
echo stationary | sudo tee /sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profile
# stationary≈60% / balanced≈80% / high_capacity=充满
```

续航另两件事：`sudo mctl profile set quiet` + `sudo systemctl enable --now tlp`，加内核参数 `mem_sleep_default=deep`（默认 s2idle 费电）。

## 11. 踩坑清单（按出现概率排序）

| 症状 | 原因 | 处理 |
| --- | --- | --- |
| 驱动装了但风扇/键盘都控不了 | clevo_wmi / qc71_laptop 和 uniwill_wmi 抢 EC | 加 `module_blacklist=clevo_wmi,qc71_laptop` |
| `dmesg` 狂刷 `uw ec read timeout` | 同上，EC 访问冲突 | 同上 |
| 模块压根没加载 | Secure Boot 未关，DKMS 模块无签名 | BIOS 关 Secure Boot，或 `sbctl` 自签 |
| 合盖/睡眠后自己醒来 | EC 误报唤醒 | 内核参数 `acpi.ec_no_wakeup=1` |
| 内屏花屏/闪烁/撕裂 | amdgpu PSR 问题（Rembrandt） | 内核参数 `amdgpu.dcdebugmask=0x10` |
| 拔电掉电飞快 | tccd 重置 CPU 调度器为 performance | `systemctl stop tccd`，改用 `power-profiles-daemon` |
| TCC 界面空白、风扇不可调 | 内核头包没装，tuxedo_io 没编出来 | 装 headers 后 `dkms autoinstall` 再重启 |
| GNOME 下找不到 TCC 入口 | 托盘图标需要扩展 | 装 `AppIndicator and KStatusNotifierItem Support` |
| 有线网口没网 | 批次是裕太微 YT6801 | `yay -S yt6801-dkms` |
| 睡眠耗电 | 默认 s2idle | 内核参数 `mem_sleep_default=deep` |

内核参数写在 `/etc/default/grub`（systemd-boot 改 `/boot/loader/entries/*.conf` 的 `options` 行）。**按需添加，别无脑全抄。**

```bash
GRUB_CMDLINE_LINUX_DEFAULT="quiet splash acpi.ec_no_wakeup=1 mem_sleep_default=deep nvidia_drm.modeset=1"
sudo grub-mkconfig -o /boot/grub/grub.cfg
```

## 12. mctl 用法

```bash
mctl detect        # 探测这台机器到底哪些功能能控（重启后第一个跑这个）
mctl status        # 当前状态总览
mctl profile [get|set] <quiet|balanced|performance>
mctl fan           # 风扇转速 + 温度
mctl kbd brightness <0-max>
mctl kbd color <RRGGBB>
mctl battery limit <60|80|100>
mctl gpu           # 显卡状态与切换说明
mctl uefi          # 读原厂控制台存在固件里的设置（需 sudo）
mctl ecmap         # EC RAM 实测地址地图（只读参考）
```

`mctl detect` 的输出请贴回来——不同批次蛟龙16K 的 EC 寄存器有差异，看到真实探测结果才能做精确适配。

## 13. 硬件之外的「用不了」（生态速查）

- **微信 / QQ**：`yay -S wechat-universal-bmap` / `linuxqq`。功能比 Windows 版少，但能用。
- **Office**：`onlyoffice-desktopeditors` 或 WPS Linux 版；重度 VBA/宏留在 Windows。
- **网盘**：坚果云有 Linux 客户端；百度网盘 `baidunetdisk-bin` 或网页版。
- **游戏**：Steam + Proton 大部分能跑；带反作弊的（部分腾讯系、Valorant 类）跑不了，硬限制。
- **Windows 独占软件**：留双系统，或 `virt-manager` + KVM 显卡直通（有独显，可行但配置麻烦）。

## 14. 验证清单（装完逐条打勾）

- [ ] `zcat /proc/config.gz | grep -i UNIWILL` 能看到 `CONFIG_UNIWILL_LAPTOP=m`
- [ ] `lsmod | grep uniwill` 有输出（force=1 加载成功）
- [ ] `/sys/bus/platform/devices/` 下有 `INOU0000:00`
- [ ] `dmesg | grep -i ec` 没有 timeout 刷屏
- [ ] `mctl detect` 里 uniwill / kbd / profile 有后端
- [ ] 键盘背光快捷键能切亮度
- [ ] `mctl profile set performance` 后再 `get` 值确实变了
- [ ] 合盖睡眠后不会自己醒
- [ ] `prime-run glxinfo` 显示 NVIDIA
- [ ] 拔电续航能接受（不行就停 tccd 试试）

---

# 第五部分 · 进阶

## 15. 为什么不用自己从 Windows 控制中心「提取」

TUXEDO 和社区已经把这件事做完了：控制台每次操作本质都是往 EC 寄存器写值，这些地址和含义已被逆向并写进 `tuxedo-drivers` 内核模块，TCC 只是图形外壳。自己重新提取等于重复造轮子，而且直接读写 EC **有刷砖风险**。我们要做的只是在这套成熟方案之上补齐蛟龙16K 的差异点——这正是 `mctl detect` 的意义。

## 16. 给内核补 DMI 条目（根治 force=1）

蛟龙16K 要 force 纯粹因为厂商没提交 DMI 信息。白名单在 `drivers/platform/x86/uniwill/uniwill-acpi.c` 的 `uniwill_dmi_table`，加一条：

```c
{
    .matches = {
        DMI_MATCH(DMI_SYS_VENDOR, "MECHREVO"),
        DMI_EXACT_MATCH(DMI_BOARD_NAME, "GM6BG0Q"),
    },
},
```

提交流程：`git send-email` 发 `platform-driver-x86@vger.kernel.org`，CC 驱动作者 Armin Wolf `<W_Armin@gmx.de>` 和 `Werner Sembach <wse@tuxedocomputers.com>`。嫌麻烦就去 [bugzilla.kernel.org](https://bugzilla.kernel.org) 提请求。

## 17. 缺口清单与现场补齐指南

本手册不是完美的——以下内容**还没拿到**。但每一个都配好了"到时候怎么补"的具体操作，装完系统照着跑一遍，缺口就自动填上。**遇到手册没写的情况，优先跑 17.4 的万能探测组，把输出发给 AI。**

### 17.1 Windows 侧拿不到、Linux 侧一条命令拿到的

**UEFI 变量实际值**（Windows 被权限策略挡死）：

```bash
sudo cat "/sys/firmware/efi/efivars/UniWillVariable-9f33f85c-13ca-4fd1-9c4a-96217722c593" | xxd
# 或 sudo mctl uefi（格式化输出 + 字段名说明）
```

拿到 dump 后对照第 4 节字段名（PowerMode / BatteryLimitation / ChargeMin-MaxLimit / RGBKeyboard…）逐字节猜语义；配合 17.2 的改设置 diff 可精确定位。

**ACPI DSDT 表**（Windows 注册表解析失败的那块）：

```bash
sudo pacman -S iasl
sudo cat /sys/firmware/acpi/tables/DSDT > dsdt.aml
iasl -d dsdt.aml        # 得到 dsdt.dsl，可读的 ACPI 源码
grep -iE "INOU|EC0|Q16|MBT" dsdt.dsl | head -30
```

**ryzenadj 字段映射校准**（目前是推测）：

1. Windows 下切到某个性能模式（记下是哪档）
2. 重启进 Linux，`sudo ryzenadj -i` 读实际值
3. 和第 2 节表格对数：35W 档 stapm/fast/slow 谁是 35000，映射即实锤
4. EC 里 0x046A 的 SPL 值（第 6.2 节）可交叉验证

### 17.2 需要"改设置 + 前后 diff"才能定位的（Windows 侧还剩三个）

复用 `admin/step5-scan.ps1`（每行即落盘，安全）：

| 实验 | 操作 | 扫描区域 |
|---|---|---|
| 键盘灯效 diff | 控制台改一次键盘颜色/亮度，前后各扫一次 | `0x0780-0x07BF` |
| 充电阈值 diff | 充电上限 100%↔80% 切一次，前后各扫一次 | `0x0430-0x04A0` |
| cTGP diff | 115W↔140W 切一次，前后各扫一次 | `0x1800-0x1820` |

```powershell
# 例（会弹 UAC）：
& "C:\Users\li\WorkBuddy\2026-09-28-23-20-34\admin\step5-scan.ps1" -RegionsJson '[[1920,1983]]'
```

同一个设置改过去再改回来（三快照 A==C≠B 判据，同第 6.4 节），就能锁定确切的寄存器。**顺手做掉这三个，Linux 侧连 RGB/充电的底层语义都齐了。**

### 17.3 只有装完 Arch 才能验证的

- [ ] `mctl detect` 输出 → 确认 force=1 后哪些特性真的可用（**发回给 AI 做精确适配**）
- [ ] `platform_profile` 切档时 EC 曲线表是否同步重写（复用 6.4 节方法，Linux 侧用 tuxedo 接口或模式切换前后 `mctl ecmap` 对照）
- [ ] tccd 耗电 bug 是否复现（掉电快就 `systemctl stop tccd`）
- [ ] 睡眠唤醒（合盖 → `acpi.ec_no_wakeup=1` 是否需要）
- [ ] 有线网卡批次（`lspci -nn | grep -i ethernet`，裕太微 YT6801 则装 `yt6801-dkms`）

### 17.4 万能探测组（遇到任何手册没覆盖的情况，先跑这个）

```bash
echo "=== PCI ===";        lspci -nn
echo "=== USB ===";        lsusb
echo "=== DMI ===";        cat /sys/class/dmi/id/{bios_vendor,bios_version,board_vendor,board_name,product_name} 2>/dev/null
echo "=== 驱动加载 ===";    lsmod | grep -iE 'uniwill|tuxedo|clevo|nvidia|amdgpu'
echo "=== 内核消息 ===";    sudo dmesg | grep -iE 'uniwill|ec |ec:|inou' | tail -30
echo "=== 温度风扇 ===";    sensors
echo "=== hwmon ===";      grep -H . /sys/class/hwmon/hwmon*/name 2>/dev/null
echo "=== 电池 ===";        grep -H . /sys/class/power_supply/*/capacity /sys/class/power_supply/*/status 2>/dev/null
echo "=== 充电阈值 ===";    cat /sys/class/power_supply/BAT*/charge_control_{start,end}_threshold 2>/dev/null
echo "=== LED ===";        ls /sys/class/leds/ 2>/dev/null
echo "=== 性能档 ===";      cat /sys/firmware/acpi/platform_profile_choices /sys/firmware/acpi/platform_profile 2>/dev/null
echo "=== uniwill ===";    ls /sys/bus/platform/devices/ | grep -i INOU
```

把整段输出贴给任何 AI + 本手册，就能继续诊断，不依赖原始对话。

### 17.5 应急回滚（force=1 出问题时）

```bash
sudo modprobe -r uniwill-laptop                    # 立即卸载
echo 'options uniwill-laptop force=0' | sudo tee /etc/modprobe.d/uniwill.conf   # 或直接删配置
sudo rm /etc/modprobe.d/uniwill.conf /etc/modules-load.d/uniwill.conf           # 彻底移除自加载
```

最坏情况（系统起不来）：Live USB 启动 → 挂载根分区 → chroot 里删上面两个文件。**EC 写入没有发生过（全程只读），硬件不会有任何持久影响。**

## 18. 给 AI 的执行指令（可直接复制）

下面是自包含上下文，粘给任何 AI 即可继续推进，不需要再看原始对话：

```
目标机器：MECHREVO 蛟龙16K（Jiaolong16K Series GM6BG0Q）
  - 主板 MECHREVO GM6BG0Q（同方 TongFang 模具，RTX 4060 版）
  - BIOS: AMI N.1.19MRO15 (2023-06-29)
  - CPU: AMD Ryzen 7 7735H (Rembrandt) + Radeon 680M 核显
  - 独显: NVIDIA RTX 4060 Laptop (140W, GN21)
  - 屏幕: 16" 2560x1600 165Hz
  - 目标系统: Arch Linux（2026.09 ISO，内核 7.2.2），尚未安装

已确认的事实（本机 Windows 侧实测验证过，不用再查）：
  1. EC 是标准 Uniwill EC。原厂驱动 UWACPIDriver.sys 绑定 ACPI\INOU0000；
     WMI 类 AcpiTest_MULong 在 root\wmi（管理员下 10 个实例）。
  2. 主线内核 6.19+ 的 uniwill-laptop 驱动（CONFIG_UNIWILL_LAPTOP=m）就是对应驱动，
     Arch 7.2.2 自带。
  3. 该驱动用 DMI 白名单自动加载，白名单不含 MECHREVO / GM6BG0Q，
     默认 return -ENODEV 拒绝加载。必须 force=1。
  4. force 模式启用除"电池充电限制"外全部特性。
  5. 键盘失灵是 6.0~6.2 老 bug，GM6BG0Q 已进内核 irq1_edge_low_force_override
     白名单，现代内核无需补丁，不要加 i8042.nomux 之类的老参数。
  6. EC RAM 已实测（WMI GetSetULong，Data=addr|(val<<16)|(0x0100<<32)，
     返回低16位=相邻两字节 [addr] 和 [addr+1]，0xFEFEFEFE=失败），并做了
     模式切换三快照 diff（A→B→C，A==C≠B 判据，58 个寄存器可逆）：
     - 0x046A/0x046B（镜像 0x0783/0x0784）= 当前 CPU SPL（W），随模式 80↔35 翻转
     - 0x0785 = FPPT 恒 100W（四模式 JSON 全是 100）
     - 0x0F00-0x0F5F = CPU/GPU 风扇曲线表（UpT/DownT 滞回 + Duty 0-200 刻度 +
       0xFF 填充），随模式整表重写、切回完全复原
     - 0x043E/0x044F = CPU/GPU 温度；0x0744 = CPU 风扇转速（×60≈RPM，停转归0）
     - 0x1804/0x1809 = 疑似 CPU/GPU 当前功耗（随负载不随模式）
     - OperatingMode=2 ↔ 80W 性能档、OperatingMode=0 ↔ 35W 静音档
       （索引≠Mode{N+1} 文件号）
     - 0x0780-0x07BF 键盘背光区、0x0430-0x04A0 电池区语义未完全定位
     - 风扇曲线实测值：80W 档 CPU UpT 10级 53-87°C/Duty200，35W 档 8级 53-69°C/Duty110
  7. UEFI 变量 UniWillVariable（GUID 9f33f85c-13ca-4fd1-9c4a-96217722c593）
     Windows 侧读不到（令牌无 SeSystemEnvironmentPrivilege），
     Linux 侧 root 直接读 /sys/firmware/efi/efivars/ 即可。
  8. 原厂四模式参数（UserPofiles JSON）：SPL/SPPT=65/35/80/65W，FPPT=100W，
     TCC=99°C，cTGP=115/关/140/115W，Mode4 开 DynamicBoost，
     Mode3 GPU 偏移 +100/+500。

开工流程：
  - 机器已装好 Arch 的情况下：先向用户要一段本机探测输出再动手，不要凭空给命令：
      lspci -nn
      lsmod | grep -iE 'uniwill|nvidia|amdgpu'
      sudo dmesg | grep -iE 'uniwill|ec |inou'
      ls /sys/firmware/efi/efivars/ | grep -i uniwill
      cat /sys/class/platform_profile 2>/dev/null || ls /sys/firmware/acpi/platform_profile*
    根据输出确认驱动加载状态，再决定从哪一步继续。
  - 尚未装系统时：从"请帮我做"第一条开始即可。

请帮我做：
  - 生成这台机器安装 Arch 的完整步骤（含分区、显卡驱动、桌面环境）
  - 配置 nvidia + prime 按需渲染
  - 让 uniwill-laptop 以 force=1 开机自动加载
  - 配好键盘背光、性能档位、风扇监控
  - 给出睡眠唤醒、EC 冲突、amd-pstate 耗电这几个已知坑的处理

约束：
  - 不要推荐从 Windows 控制中心逆向 EC 寄存器，有刷砖风险，内核驱动已封装
  - 不要加 i8042.nomux / acpi_osi 之类的过时内核参数
  - 不确定的地方标注"待实测确认"，不要编造 sysfs 路径
  - 修改 GRUB 配置前先给出 diff 并说明风险
  - 绝不向 EC 寄存器裸写（WMI/sysfs 直写），写操作一律走内核驱动接口
```

---

## 附：一句话总结

原厂控制台对蛟龙16K 做的事，拆开看就是三件：**设 CPU 功耗墙（ryzenadj 能干）、设 GPU cTGP（内核 ctgp_offset 能干）、切模式让 EC 自己管风扇（platform_profile 能干）**。EC RAM 实测已经把它的每个动作都看在了眼里——风扇曲线、SPL、温度、功耗的地址全部落地。原厂控制台没有任何魔法，它做的事全部有 Linux 等价物。
