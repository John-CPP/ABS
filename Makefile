# ABS / absgui local build helpers
#
#   make fast          # quick optimized build (no LTO) → target/fast/
#   make install-fast  # build fast + install binaries and desktop assets
#   make release       # production build (thin LTO) → target/release/
#   make install       # build release + install binaries + desktop assets
#   make aur           # makepkg -s, then install abs (asks about AbsGui on first install)
#
# Override install root:  make install-fast PREFIX=$HOME/.local
# Or:                     make install DESTDIR=/tmp/stage PREFIX=/usr

PREFIX  ?= /usr
BINDIR  ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
DESTDIR ?=

CARGO  ?= cargo
SUDO   ?= sudo
INSTALL ?= install

FAST_DIR    := target/fast
RELEASE_DIR := target/release
ICON_SIZES  := 32 48 64 128 256 512

.PHONY: help fast release test test-fast install-desktop-assets install-fast install aur clean

help:
	@echo "Targets:"
	@echo "  fast          Build abs + absgui with --profile fast (no LTO)"
	@echo "  install-fast  fast + install binaries and desktop/icon to $(DESTDIR)$(BINDIR)"
	@echo "  release       Build abs + absgui with --release (thin LTO)"
	@echo "  install       release + install binaries and desktop/PGO assets"
	@echo "  test          cargo test (debug)"
	@echo "  test-fast     cargo test --profile fast"
	@echo "  aur           cd aur && ./install.sh (asks about AbsGui when there is no abs.toml)"
	@echo "  clean         cargo clean"
	@echo ""
	@echo "Cargo aliases:  cargo fast   |  cargo rel"
	@echo "PREFIX=$(PREFIX)  DESTDIR=$(DESTDIR)"

fast:
	$(CARGO) build --profile fast

release:
	$(CARGO) build --release

test:
	$(CARGO) test

test-fast:
	$(CARGO) test --profile fast

install-desktop-assets:
	$(foreach s,$(ICON_SIZES),$(SUDO) $(INSTALL) -Dm644 absgui/assets/icons/icon_$(s).png $(DESTDIR)$(DATADIR)/icons/hicolor/$(s)x$(s)/apps/absgui.png;)
	$(SUDO) $(INSTALL) -Dm644 absgui/absgui.desktop $(DESTDIR)$(DATADIR)/applications/absgui.desktop
	-$(SUDO) update-desktop-database $(DESTDIR)$(DATADIR)/applications 2>/dev/null || true

install-fast: fast
	$(SUDO) $(INSTALL) -Dm755 $(FAST_DIR)/abs $(DESTDIR)$(BINDIR)/abs
	$(SUDO) $(INSTALL) -Dm755 $(FAST_DIR)/absgui $(DESTDIR)$(BINDIR)/absgui
	$(MAKE) install-desktop-assets
	@echo "Installed fast build to $(DESTDIR)$(BINDIR)/{abs,absgui}"

install: release
	$(SUDO) $(INSTALL) -Dm755 $(RELEASE_DIR)/abs $(DESTDIR)$(BINDIR)/abs
	$(SUDO) $(INSTALL) -Dm755 $(RELEASE_DIR)/absgui $(DESTDIR)$(BINDIR)/absgui
	$(SUDO) $(INSTALL) -Dm755 assets/pgo-benchmark.sh $(DESTDIR)$(DATADIR)/abs/pgo-benchmark.sh
	$(MAKE) install-desktop-assets
	@echo "Installed release build to $(DESTDIR)$(BINDIR)/{abs,absgui}"

aur:
	cd aur && ./install.sh

clean:
	$(CARGO) clean
