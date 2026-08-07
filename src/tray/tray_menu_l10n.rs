/// Tray-menu localization, covering the same languages the web frontend supports
/// (gephgui/src/lib/l10n.ts): en, zh-CN, zh-TW, fa, ar, ru, es, uk. The
/// Connect/Disconnect wording matches the frontend's own `l10n.csv`. The tray is
/// a native element built once at startup, so — like the frontend's
/// `detectNearestBrowserLocale` — we pick the nearest language from the OS locale
/// via `sys-locale`, falling back to English.

#[derive(Clone, Copy)]
pub(super) enum Lang {
    En,
    ZhCn,
    ZhTw,
    Fa,
    Ar,
    Ru,
    Es,
    Uk,
}

pub(super) fn detect() -> Lang {
    let locale = sys_locale::get_locale().unwrap_or_default().to_lowercase();
    if locale.starts_with("zh") {
        // Traditional Chinese for Taiwan/Hong Kong/Macau or an explicit
        // `Hant` script subtag; Simplified otherwise.
        if ["tw", "hk", "mo", "hant"].iter().any(|t| locale.contains(t)) {
            Lang::ZhTw
        } else {
            Lang::ZhCn
        }
    } else {
        match locale.split(['-', '_']).next().unwrap_or("") {
            "fa" => Lang::Fa,
            "ar" => Lang::Ar,
            "ru" => Lang::Ru,
            "es" => Lang::Es,
            "uk" => Lang::Uk,
            _ => Lang::En,
        }
    }
}

pub(super) struct Labels {
    pub show: &'static str,
    pub connect: &'static str,
    pub disconnect: &'static str,
    pub cancel: &'static str,
    pub quit: &'static str,
    pub disconnected: &'static str,
    pub connecting: &'static str,
    pub connected: &'static str,
    pub global_vpn: &'static str,
    pub split_tunnel: &'static str,
    pub select_server: &'static str,
    pub server: &'static str,
    pub no_available_servers: &'static str,
    pub auto: &'static str,
    pub core: &'static str,
    pub streaming: &'static str,
}

pub(super) fn labels(lang: Lang) -> Labels {
    match lang {
        Lang::En => Labels {
            show: "Show Geph",
            connect: "Connect",
            disconnect: "Disconnect",
            cancel: "Cancel",
            quit: "Quit",
            disconnected: "Disconnected",
            connecting: "Connecting...",
            connected: "Connected",
            global_vpn: "Global VPN",
            split_tunnel: "Exclude PRC Traffic",
            select_server: "Select Server",
            server: "Server",
            no_available_servers: "No Available Servers",
            auto: "Auto",
            core: "Core",
            streaming: "Streaming",
        },
        Lang::ZhCn => Labels {
            show: "显示 Geph",
            connect: "连接",
            disconnect: "断开",
            cancel: "取消",
            quit: "退出",
            disconnected: "已断开",
            connecting: "连接中...",
            connected: "已连接",
            global_vpn: "全局VPN",
            split_tunnel: "排除中国大陆流量",
            select_server: "选择服务器",
            server: "服务器",
            no_available_servers: "无可用服务器",
            auto: "自动",
            core: "核心",
            streaming: "流媒体",
        },
        Lang::ZhTw => Labels {
            show: "顯示 Geph",
            connect: "連接",
            disconnect: "斷開",
            cancel: "取消",
            quit: "結束",
            disconnected: "未連線",
            connecting: "連線中...",
            connected: "已連線",
            global_vpn: "全域VPN",
            split_tunnel: "排除中國流量",
            select_server: "選擇伺服器",
            server: "伺服器",
            no_available_servers: "沒有可用伺服器",
            auto: "自動",
            core: "核心",
            streaming: "串流",
        },
        Lang::Fa => Labels {
            show: "نمایش Geph",
            connect: "اتصال",
            disconnect: "قطع اتصال",
            cancel: "لغو",
            quit: "خروج",
            disconnected: "قطع شده",
            connecting: "در حال اتصال...",
            connected: "متصل",
            global_vpn: "VPN سراسری",
            split_tunnel: "استثنای ترافیک چین",
            select_server: "انتخاب سرور",
            server: "سرور",
            no_available_servers: "هیچ سروری در دسترس نیست",
            auto: "خودکار",
            core: "هسته",
            streaming: "پخش جریانی",
        },
        Lang::Ar => Labels {
            show: "إظهار Geph",
            connect: "اتصال",
            disconnect: "قطع الاتصال",
            cancel: "إلغاء",
            quit: "خروج",
            disconnected: "غير متصل",
            connecting: "جارٍ الاتصال...",
            connected: "متصل",
            global_vpn: "VPN شامل",
            split_tunnel: "استثناء حركة مرور الصين",
            select_server: "اختيار الخادم",
            server: "الخادم",
            no_available_servers: "لا توجد خوادم متاحة",
            auto: "تلقائي",
            core: "أساسي",
            streaming: "البث",
        },
        Lang::Ru => Labels {
            show: "Показать Geph",
            connect: "Подключить",
            disconnect: "Отключить",
            cancel: "Отмена",
            quit: "Выход",
            disconnected: "Не подключено",
            connecting: "Подключение...",
            connected: "Подключено",
            global_vpn: "Глобальный VPN",
            split_tunnel: "Исключить трафик КНР",
            select_server: "Выбрать сервер",
            server: "Сервер",
            no_available_servers: "Нет доступных серверов",
            auto: "Авто",
            core: "Основной",
            streaming: "Потоковое видео",
        },
        Lang::Es => Labels {
            show: "Mostrar Geph",
            connect: "Conectar",
            disconnect: "Desconectar",
            cancel: "Cancelar",
            quit: "Salir",
            disconnected: "Desconectado",
            connecting: "Conectando...",
            connected: "CConectado",
            global_vpn: "VPN global",
            split_tunnel: "Excluir tráfico de China",
            select_server: "Seleccionar servidor",
            server: "Servidor",
            no_available_servers: "No hay servidores disponibles",
            auto: "Automático",
            core: "Núcleo",
            streaming: "Transmisión",
        },
        Lang::Uk => Labels {
            show: "Показати Geph",
            connect: "Підключити",
            disconnect: "Відключити",
            cancel: "Скасувати",
            quit: "Вийти",
            disconnected: "Не підключено",
            connecting: "Підключення...",
            connected: "Підключено",
            global_vpn: "Глобальний VPN",
            split_tunnel: "Виключити трафік КНР",
            select_server: "Вибрати сервер",
            server: "Сервер",
            no_available_servers: "Немає доступних серверів",
            auto: "Авто",
            core: "Основний",
            streaming: "Потокове відео",
        },
    }
}

pub(super) fn current_labels() -> Labels {
    labels(detect())
}