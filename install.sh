#!/usr/bin/env bash
# 蛟龙16K 控制中心 安装脚本。用法：./install.sh [--uninstall]
# 以普通用户运行，需要 root 的步骤会自己调用 sudo。
set -euo pipefail
cd "$(dirname "$0")"

ME=$(id -un); MYUID=$(id -u)
BOARD=$(cat /sys/class/dmi/id/board_name 2>/dev/null || true)

if [ "${1:-}" = "--uninstall" ]; then
  sudo systemctl disable --now jiaolong-hotkeyd.service ryzenadj-profile.service 2>/dev/null || true
  sudo rm -f /etc/systemd/system/{jiaolong-hotkeyd,ryzenadj-profile}.service \
             /etc/polkit-1/rules.d/50-jiaolong.rules \
             /usr/local/libexec/jiaolong-helper /usr/local/libexec/jiaolong-hotkeyd \
             /usr/local/bin/ryzenadj-profile /usr/local/bin/jiaolong-ctl \
             /etc/modules-load.d/acpi_call.conf
  sudo rm -rf /etc/jiaolong
  rm -f ~/.local/share/applications/jiaolong-ctl.desktop
  sudo systemctl daemon-reload
  echo "已卸载（不会改动 uniwill 驱动配置，也不会卸载 ryzenadj / acpi_call 软件包）"
  exit 0
fi

if [ "$BOARD" != "GM6BG0Q" ]; then
  echo "警告：本机主板是 '$BOARD'，不是 GM6BG0Q（蛟龙16K）。" >&2
  echo "EC 档位寄存器写入只在 GM6BG0Q 上生效，其余功能可能不适用。" >&2
  read -rp "仍要继续安装？[y/N] " a; [ "$a" = y ] || exit 1
fi

echo "==> 依赖"
sudo pacman -S --needed --noconfirm ryzenadj acpi_call libnotify gtk4 polkit
command -v cargo >/dev/null || sudo pacman -S --needed --noconfirm rust

echo "==> 编译 GUI"
cargo build --release

echo "==> 安装文件（用户 $ME, uid $MYUID）"
sudo install -Dm755 target/release/jiaolong-ctl /usr/local/bin/jiaolong-ctl
sudo install -Dm755 system/ryzenadj-profile /usr/local/bin/ryzenadj-profile
sudo install -Dm755 system/jiaolong-helper /usr/local/libexec/jiaolong-helper
sed "s/@USER@/$ME/g; s/@UID@/$MYUID/g" system/jiaolong-hotkeyd | sudo tee /usr/local/libexec/jiaolong-hotkeyd >/dev/null
sudo chmod 755 /usr/local/libexec/jiaolong-hotkeyd
sed "s/@USER@/$ME/g" system/50-jiaolong.rules | sudo tee /etc/polkit-1/rules.d/50-jiaolong.rules >/dev/null
sudo chmod 644 /etc/polkit-1/rules.d/50-jiaolong.rules
sudo install -Dm644 system/ryzenadj-profile.service /etc/systemd/system/ryzenadj-profile.service
sudo install -Dm644 system/jiaolong-hotkeyd.service /etc/systemd/system/jiaolong-hotkeyd.service
echo acpi_call | sudo tee /etc/modules-load.d/acpi_call.conf >/dev/null
install -Dm644 jiaolong-ctl.desktop ~/.local/share/applications/jiaolong-ctl.desktop

echo "==> 驱动与服务"
sudo modprobe acpi_call || true
if ! lsmod | grep -q '^uniwill'; then
  echo "未检测到 uniwill 驱动。尝试 force 加载（内核 >=7.3 且已带本机 DMI 条目时不需要）："
  sudo modprobe uniwill-laptop force=1 || echo "加载失败，请参阅 README「驱动」一节" >&2
fi
sudo systemctl daemon-reload
sudo systemctl enable --now ryzenadj-profile.service jiaolong-hotkeyd.service

echo "完成。运行：jiaolong-ctl（或在应用菜单里搜「蛟龙」）"
