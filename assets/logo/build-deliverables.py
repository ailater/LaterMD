#!/usr/bin/env python3
"""LaterMD 品牌图标交付流水线：多尺寸 PNG + 三平台原生格式 + 变体 SVG + icns。

源素材（AIM 标识，叶可儿设计，2026-10-09 定稿）在本目录下：
  latermd-icon.svg       渐变圆角方 + 白色 MD 字形 + 星光（应用图标，主用）
  latermd-mark.svg       仅字形、无底板（需要自绘底/单色时用）
  latermd-mark-black.svg 单色版 #0B1220（浅底）
  latermd-mark-white.svg 单色版 #FFFFFF（深底）

平台产物与消费方（谁在运行时读哪个文件）：
  png/icon-64.png            → latermd-app assets.rs 窗口图标（egui IconData）
  png/icon-256.png           → 标题栏左上角品牌标识（egui 纹理）
  windows/latermd.ico        → build.rs 经 winresource 嵌入 exe 资源段
  macOS/AppIcon.iconset      → macos-dmg.yml 组 .app 时 iconutil 合 icns
  macOS/AppIcon.icns         → 本脚本直接拼装（见下），macOS 侧备用/复核
  web/favicon.ico|favicon.svg → README / 文档站

icns 不走 iconutil（Linux 上无此工具），按 icns 容器格式直接拼：
  'icns' + u32BE(总长) + 重复的 [4 字节 OSType + u32BE(数据长+8) + PNG 数据]。
PNG 负载用 icp4/5/6(16/32/64) + ic07/08/09/10(128/256/512/1024)，
macOS 10.7+ 全部接受 PNG 负载（更早的 arnnd/ic04 路线不必走）。
macOS CI 侧仍以系统 iconutil 为准，本文件是跨平台构建的可复现副本。

依赖：rsvg-convert（渲染 SVG）+ ImageMagick convert（合成多尺寸 ICO）。
"""
import os
import shutil
import struct
import subprocess

ROOT = os.path.dirname(os.path.abspath(__file__))
SRC_ICON = os.path.join(ROOT, "latermd-icon.svg")
DIST = os.path.join(ROOT, "deliverables")
PNG_DIR = os.path.join(DIST, "png")
SVG_DIR = os.path.join(DIST, "svg")
WIN_DIR = os.path.join(DIST, "windows")
MAC_DIR = os.path.join(DIST, "macOS")
WEB_DIR = os.path.join(DIST, "web")

# PNG 交付尺寸。64 不可改：crates/latermd-app/src/assets.rs 的守门测试
# window_icon_decodes_to_64x64_rgba 钉死窗口图标为 64×64，改尺寸即红。
# 24/36 是为 Linux hicolor 目录补的（见linux-deb.yml 的安装档位）——
# 那边要装 16/24/32/48/64/128/256/512，缺档会让 install 当场失败。
SIZES = [1024, 512, 256, 192, 128, 96, 64, 48, 36, 32, 24, 16]

# macOS iconset：(输出名, 像素边长)。命名与 @2x 语义由 Apple 规定，勿改。
# 各档**不经全出血直渲**：按 Apple Big Sur 图标网格（1024 画布、内容
# 824×824、四周 100px 透明边距）缩放居中 —— macOS 的 Dock/访达按画布
# 等比摆位，全出血图标会比同排其他应用明显大一圈（2026-10-24 mac 真机
# 反馈）。其他平台（Windows ICO / Linux hicolor / web favicon）维持
# 全出血：那些系统按内容本身摆位，不留边。
MACOS_INNER_SCALE = 824 / 1024
ICONSET = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

# icns 块类型 -> 像素边长。16/32/64 走 icp4/5/6，128 以上走 ic07..ic10。
ICNS_TYPES = [
    ("icp4", 16),
    ("icp5", 32),
    ("icp6", 64),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
]

# Windows ICO 内嵌尺寸。资源管理器/任务管理器按 DPI 选档，16 是必下档。
ICO_SIZES = [16, 32, 48, 64, 128, 256]

# web favicon 的进制 PNG 档位；另有 SVG 矢量版给现代浏览器。
FAVICON_PNG_SIZES = [16, 32, 48]

_rendered = {}


def render(src, out, width, height=None):
    """SVG -> PNG。rsvg-convert 保证渐变与曲线不失真（ImageMagick 内建
    SVG 渲染器对 linearGradient 支持不全，故不用它渲源图）。"""
    height = height or width
    os.makedirs(os.path.dirname(out), exist_ok=True)
    subprocess.run(
        ["rsvg-convert", "-w", str(width), "-h", str(height), src, "-o", out],
        check=True,
        capture_output=True,
    )


def render_padded(src, out, width, height=None, inner=1.0):
    """SVG -> 带透明边距的 PNG：内容先渲成 inner*width/height，再用
    ImageMagick 居中扩到完整画布（透明底）。macOS iconset 专用
    （MACOS_INNER_SCALE），渲染路径与全出血档共用 [`render`]。"""
    height = height or width
    inner_w = max(1, round(width * inner))
    inner_h = max(1, round(height * inner))
    staged = f"{out}.inner{inner_w}.png"
    render(src, staged, inner_w, inner_h)
    subprocess.run(
        [
            "convert", staged,
            "-background", "none",
            "-gravity", "center",
            "-extent", f"{width}x{height}",
            out,
        ],
        check=True,
        capture_output=True,
    )
    os.remove(staged)


def png(size):
    """取 SIZES 档位的 PNG，缺档现渲并缓存（供 ico/icns/favicon 复用）。"""
    if size not in _rendered:
        path = os.path.join(PNG_DIR, f"icon-{size}.png")
        if not os.path.exists(path):
            render(SRC_ICON, path, size)
        _rendered[size] = path
    return _rendered[size]


def build_icns(blocks):
    """拼 icns 容器：'icns' + u32BE(总长) + [OSType + u32BE(数据长+8) + 数据]*。
    长度字段含自身 8 字节头 —— 这是 Apple 的约定，写成纯数据长会让
    iconutil/Cocoa 解析错位。"""
    body = b""
    for ostype, path in blocks:
        data = open(path, "rb").read()
        body += ostype.encode("ascii") + struct.pack(">I", len(data) + 8) + data
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def main():
    # 只清生成的子目录，保留源 SVG 与本脚本
    for path in (PNG_DIR, SVG_DIR, WIN_DIR, MAC_DIR, WEB_DIR):
        shutil.rmtree(path, ignore_errors=True)
    os.makedirs(PNG_DIR, exist_ok=True)

    # --- PNG 多尺寸 ---
    for size in SIZES:
        render(SRC_ICON, os.path.join(PNG_DIR, f"icon-{size}.png"), size)

    # --- 变体 SVG（网页 / 文档 / 自绘底场景直接引用，不重绘一遍）---
    os.makedirs(SVG_DIR, exist_ok=True)
    shutil.copy(SRC_ICON, os.path.join(SVG_DIR, "latermd-icon.svg"))
    for name in ("latermd-mark.svg", "latermd-mark-black.svg", "latermd-mark-white.svg"):
        shutil.copy(os.path.join(ROOT, name), os.path.join(SVG_DIR, name))

    # --- Windows ICO：多档 PNG 合成单个 ico（资源管理器按 DPI 选档）---
    os.makedirs(WIN_DIR, exist_ok=True)
    ico = os.path.join(WIN_DIR, "latermd.ico")
    subprocess.run(
        ["convert"] + [png(s) for s in ICO_SIZES] + [ico],
        check=True,
        capture_output=True,
    )

    # --- macOS iconset + icns ---
    # 内容按 Apple 图标网格留边（MACOS_INNER_SCALE）；icns 直接吃 iconset
    # 的留边档（ICNS_TYPES 的七个尺寸 ICONSET 全覆盖），保证两条 macOS
    # 产物同一份像素。
    iconset_dir = os.path.join(MAC_DIR, "AppIcon.iconset")
    os.makedirs(iconset_dir, exist_ok=True)
    iconset_by_size = {}
    for name, size in ICONSET:
        path = os.path.join(iconset_dir, name)
        render_padded(SRC_ICON, path, size, inner=MACOS_INNER_SCALE)
        iconset_by_size[size] = path
    with open(os.path.join(MAC_DIR, "AppIcon.icns"), "wb") as fh:
        fh.write(build_icns([(t, iconset_by_size[s]) for t, s in ICNS_TYPES]))

    # --- web favicon ---
    os.makedirs(WEB_DIR, exist_ok=True)
    subprocess.run(
        ["convert"] + [png(s) for s in sorted(FAVICON_PNG_SIZES)]
        + [os.path.join(WEB_DIR, "favicon.ico")],
        check=True,
        capture_output=True,
    )
    # favicon.svg 是矢量副本，不是「把 32px PNG 改名成 .svg」（旧脚本踩过这坑）
    shutil.copy(SRC_ICON, os.path.join(WEB_DIR, "favicon.svg"))
    render(SRC_ICON, os.path.join(WEB_DIR, "apple-touch-icon.png"), 180)

    # --- 自检：把产物打出来，尺寸漂移一眼可见 ---
    for size in SIZES:
        path = os.path.join(PNG_DIR, f"icon-{size}.png")
        print(f"png/icon-{size}.png".ljust(38), f"{os.path.getsize(path) / 1024:>7.1f} KB")
    for name in ("windows/latermd.ico", "macOS/AppIcon.icns", "web/favicon.ico"):
        path = os.path.join(DIST, name)
        print(name.ljust(38), f"{os.path.getsize(path) / 1024:>7.1f} KB")
    print(f"macOS/AppIcon.iconset{'':<22} {len(ICONSET)} files")


if __name__ == "__main__":
    main()