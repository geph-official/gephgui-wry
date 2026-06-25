#!/usr/bin/env bash
set -e

# Change directory to the root of the workspace (relative to the scripts/ folder)
cd "$(dirname "$0")/.."

# Clear previous build directory
rm -rf build
mkdir -p build

echo "=== Step 1: Building Frontend ==="
cd gephgui
bun run build
cd ..

echo "=== Step 2: Building Backend ==="
cargo build --release

echo "=== Step 3: Initializing submodules and building pac-cmd ==="
git submodule update --init --recursive
cd pac-cmd
make
cd ..

echo "=== Step 4: Preparing temporary files ==="
mkdir -p build/sources
cp target/release/gephgui-wry build/sources/gephgui-wry
cp pac-cmd/binaries/linux_amd64/pac build/sources/pac
cp src/logo-naked.png build/sources/gephgui-wry.png

# Create desktop entry
cat << 'EOF' > build/sources/gephgui-wry.desktop
[Desktop Entry]
Name=Geph
Comment=Censorship circumvention system
Exec=gephgui-wry
Icon=gephgui-wry
Terminal=false
Type=Application
Categories=Network;
EOF

echo "=== Step 5: Building DEB Package ==="
mkdir -p build/deb/DEBIAN
mkdir -p build/deb/usr/bin
mkdir -p build/deb/usr/share/applications
mkdir -p build/deb/usr/share/pixmaps

cp build/sources/gephgui-wry build/deb/usr/bin/gephgui-wry
cp build/sources/pac build/deb/usr/bin/pac
cp build/sources/gephgui-wry.png build/deb/usr/share/pixmaps/gephgui-wry.png
cp build/sources/gephgui-wry.desktop build/deb/usr/share/applications/gephgui-wry.desktop

cat << 'EOF' > build/deb/DEBIAN/control
Package: gephgui-wry
Version: 0.0.1
Section: net
Priority: optional
Architecture: amd64
Depends: libgtk-3-0, libwebkit2gtk-4.1-0 | libwebkit2gtk-4.0-37
Maintainer: Geph Maintainers <support@geph.io>
Description: Desktop GUI for Geph
 Wry-based desktop GUI client for the Geph censorship circumvention system.
EOF

dpkg-deb --build build/deb build/gephgui-wry_0.0.1_amd64.deb

echo "=== Step 6: Building RPM Package ==="
mkdir -p build/rpm/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT}
cp build/sources/* build/rpm/SOURCES/

rpmbuild -bb \
  --define "_topdir $(pwd)/build/rpm" \
  --define "_sourcedir $(pwd)/build/rpm/SOURCES" \
  --define "_builddir $(pwd)/build/rpm/BUILD" \
  --define "_srcrpmdir $(pwd)/build/rpm/SRPMS" \
  --define "_rpmdir $(pwd)/build/rpm/RPMS" \
  --define "_buildrootdir $(pwd)/build/rpm/BUILDROOT" \
  scripts/gephgui-wry.spec

# Copy generated RPM to the build root directory
cp build/rpm/RPMS/x86_64/*.rpm build/

echo "=== Step 7: Preparing AppDir structure ==="
mkdir -p build/AppDir/usr/bin
mkdir -p build/AppDir/usr/share/applications
mkdir -p build/AppDir/usr/share/pixmaps

cp build/sources/gephgui-wry build/AppDir/usr/bin/gephgui-wry
cp build/sources/pac build/AppDir/usr/bin/pac
cp build/sources/gephgui-wry.png build/AppDir/usr/share/pixmaps/gephgui-wry.png
cp build/sources/gephgui-wry.desktop build/AppDir/gephgui-wry.desktop
cp build/sources/gephgui-wry.png build/AppDir/gephgui-wry.png

# Create AppRun entrypoint
cat << 'EOF' > build/AppDir/AppRun
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
export PATH="${HERE}/usr/bin:${PATH}"
exec "${HERE}/usr/bin/gephgui-wry" "$@"
EOF
chmod +x build/AppDir/AppRun

echo "=== Step 8: Fetching appimagetool and runtime ==="
mkdir -p build/appimage
cd build/appimage

# Download official appimagetool if not already present or if it was a failed download
if [ ! -f "appimagetool-x86_64.AppImage" ] || [ $(stat -c %s appimagetool-x86_64.AppImage) -lt 1000000 ]; then
    rm -f appimagetool-x86_64.AppImage
    curl -L -o appimagetool-x86_64.AppImage https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x appimagetool-x86_64.AppImage
fi

# Download runtime-x86_64 manually
if [ ! -f "runtime-x86_64" ] || [ $(stat -c %s runtime-x86_64) -lt 500000 ]; then
    rm -f runtime-x86_64
    curl -L -o runtime-x86_64 https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64
fi
cd ../..

echo "=== Step 9: Generating AppImage ==="
export ARCH=x86_64
./build/appimage/appimagetool-x86_64.AppImage --appimage-extract-and-run --runtime-file build/appimage/runtime-x86_64 build/AppDir build/gephgui-wry-x86_64.AppImage

echo "=== Done! All packages built in build/ ==="
ls -lh build/*.deb build/*.rpm build/*.AppImage
