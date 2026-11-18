.PHONY: help test lint mac-release mac-app install-mac release

help:
	@echo "make test          运行全部测试"
	@echo "make lint          clippy（-D warnings）"
	@echo "make mac-release   构建签名 + 公证的 requestbaby.app 与 DMG（build/mac/）"
	@echo "make mac-app       只构建签名的 .app（不公证、不出 DMG，本机自用）"
	@echo "make release       发版：测试 → 本机签名公证 DMG → GitHub Release（见 docs/RELEASING.md）"
	@echo "make install-mac   安装 build/mac/requestbaby.app 到 /Applications"

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings

mac-release:
	scripts/build-mac-release.sh

mac-app:
	SKIP_NOTARIZE=1 SKIP_DMG=1 MAC_ARCH=$${MAC_ARCH:-arm64} scripts/build-mac-release.sh

install-mac:
	scripts/install-mac.sh

release:
	scripts/release.sh
