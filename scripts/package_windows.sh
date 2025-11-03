#!/usr/bin/env bash
set -euo pipefail

# package_windows.sh
# 在 Linux 环境下交叉编译或原生构建 Windows 可执行并打包前端静态文件为 zip。
# 前提依赖:
#  - rustup, cargo
#  - 交叉目标: x86_64-pc-windows-gnu 或 x86_64-pc-windows-msvc
#  - 若选择 gnu 目标, 需要安装 mingw-w64 (用于链接)
#  - zip
# 用法: ./scripts/package_windows.sh [target]
# target 默认: x86_64-pc-windows-gnu

TARGET=${1:-x86_64-pc-windows-gnu}
BUILD_DIR=target/${TARGET}/release
OUT_DIR=dist/windows
BIN_NAME=image-upload-system
ZIP_NAME=${BIN_NAME}-${TARGET}.zip

mkdir -p ${OUT_DIR}

echo "构建 Rust 可执行 (target=${TARGET})..."
# 如果目标还未安装，可尝试自动添加
if ! rustup target list | grep -q "^${TARGET} (installed)"; then
  echo "Target ${TARGET} 未安装，尝试添加..."
  rustup target add ${TARGET}
fi

# 使用 cargo 构建 release
cargo build --release --target ${TARGET}

# 生成输出目录
TEMP_PACK_DIR=$(mktemp -d)
mkdir -p ${TEMP_PACK_DIR}/frontend
mkdir -p ${TEMP_PACK_DIR}/uploads

# 复制可执行
if [ -f "${BUILD_DIR}/${BIN_NAME}.exe" ]; then
  cp "${BUILD_DIR}/${BIN_NAME}.exe" "${TEMP_PACK_DIR}/"
else
  echo "没有找到构建产物 ${BUILD_DIR}/${BIN_NAME}.exe" >&2
  exit 1
fi

# 复制前端静态文件
cp -r frontend/* ${TEMP_PACK_DIR}/frontend/

# 拷贝默认 uploads 下文件夹结构（可选，仅示例）
if [ -d uploads ]; then
  cp -r uploads ${TEMP_PACK_DIR}/uploads
fi

# 打包为 zip
pushd ${TEMP_PACK_DIR} >/dev/null
zip -r "${ZIP_NAME}" .
popd >/dev/null

mv ${TEMP_PACK_DIR}/${ZIP_NAME} ${OUT_DIR}/
rm -rf ${TEMP_PACK_DIR}

echo "打包完成: ${OUT_DIR}/${ZIP_NAME}"

# 提示
cat <<EOF
提示:
- 在 Windows 上运行可能需要额外的依赖（如 vcruntime for msvc build 或 mingw runtime for gnu build）。
- 若选择 msvc 目标，建议在 Windows 上使用 Visual Studio 的工具链构建。
EOF
