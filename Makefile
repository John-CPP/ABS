# ABS / absgui local build helpers
#
#   make fast          # quick optimized build (no LTO) → target/fast/
#   make install-fast  # build fast + install to PREFIX (default /usr)
#   make release       # production build (thin LTO) → target/release/
#   make install       # build release + install binaries + desktop assets
#   make aur           # makepkg -si from aur/ (uses release profile)
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

.PHONY: help fast release test test-fast install-fast install aur clean

help:
	@echo "Targets:"
	@echo "  fast          Build abs + absgui with --profile fast (no LTO)"
	@echo "  install-fast  fast + install binaries to $(DESTDIR)$(BINDIR)"
	@echo "  release       Build abs + absgui with --release (thin LTO)"
	@echo "  install       release + install binaries and desktop/PGO assets"
	@echo "  test          cargo test (debug)"
	@echo "  test-fast     cargo test --profile fast"
	@echo "  aur           cd aur && makepkg -si"
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

install-fast: fast
	$(SUDO) $(INSTALL) -Dm755 $(FAST_DIR)/abs $(DESTDIR)$(BINDIR)/abs
	$(SUDO) $(INSTALL) -Dm755 $(FAST_DIR)/absgui $(DESTDIR)$(BINDIR)/absgui
	@echo "Installed fast build to $(DESTDIR)$(BINDIR)/{abs,absgui}"

install: release
	$(SUDO) $(INSTALL) -Dm755 $(RELEASE_DIR)/abs $(DESTDIR)$(BINDIR)/abs
	$(SUDO) $(INSTALL) -Dm755 $(RELEASE_DIR)/absgui $(DESTDIR)$(BINDIR)/absgui
	$(SUDO) $(INSTALL) -Dm755 assets/pgo-benchmark.sh $(DESTDIR)$(DATADIR)/abs/pgo-benchmark.sh
	$(SUDO) $(INSTALL) -Dm644 absgui/assets/icon.png $(DESTDIR)$(DATADIR)/icons/hicolor/256x256/apps/absgui.png
	$(SUDO) $(INSTALL) -Dm644 absgui/absgui.desktop $(DESTDIR)$(DATADIR)/applications/absgui.desktop
	@echo "Installed release build to $(DESTDIR)$(BINDIR)/{abs,absgui}"

aur:
	cd aur && makepkg -si

clean:
	$(CARGO) clean
