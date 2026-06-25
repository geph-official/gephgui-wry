Name:           gephgui-wry
Version:        0.0.1
Release:        1%{?dist}
Summary:        Desktop GUI for Geph

License:        MPL-2.0
URL:            https://github.com/geph-official/gephgui-wry
BuildArch:      x86_64
Requires:       gtk3, webkit2gtk4.1

%description
Wry-based desktop GUI client for the Geph censorship circumvention system.

%prep

%build

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/pixmaps

cp %{_sourcedir}/gephgui-wry %{buildroot}%{_bindir}/gephgui-wry
cp %{_sourcedir}/pac %{buildroot}%{_bindir}/pac
cp %{_sourcedir}/gephgui-wry.png %{buildroot}%{_datadir}/pixmaps/gephgui-wry.png
cp %{_sourcedir}/gephgui-wry.desktop %{buildroot}%{_datadir}/applications/gephgui-wry.desktop

%files
%{_bindir}/gephgui-wry
%{_bindir}/pac
%{_datadir}/pixmaps/gephgui-wry.png
%{_datadir}/applications/gephgui-wry.desktop
