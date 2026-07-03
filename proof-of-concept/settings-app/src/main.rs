use ardos_ui::{ContainerStyle, Direction, Element, WindowOptions, rsml, use_memo};
use motherboard_ardos_ui_integration::{
    MotherboardUi,
    motherboard_client::{ClientApi, ClientCallsApi},
    use_motherboard_store,
};

const SERVICE_NAME: &str = "SettingsManager";
const THEME_STORE: &str = "theme";

fn settings_app(_: ()) -> Box<dyn Element> {
    let motherboard = use_memo(
        || MotherboardUi::open().expect("failed to open /dev/services"),
        (),
    );

    let theme_data = use_motherboard_store((*motherboard).clone(), SERVICE_NAME, THEME_STORE);
    let theme = theme_data
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .unwrap_or("loading");
    let is_dark = theme == "dark";
    let next_theme = if is_dark { "light" } else { "dark" };

    let background = if is_dark {
        (0x12, 0x16, 0x1c, 0xff)
    } else {
        (0xf3, 0xf6, 0xf8, 0xff)
    };
    let panel = if is_dark {
        (0x1d, 0x24, 0x2d, 0xff)
    } else {
        (0xff, 0xff, 0xff, 0xff)
    };
    let text = if is_dark {
        (0xf4, 0xf7, 0xfa, 0xff)
    } else {
        (0x16, 0x1b, 0x22, 0xff)
    };
    let secondary_text = if is_dark {
        (0xa8, 0xb3, 0xc2, 0xff)
    } else {
        (0x5e, 0x69, 0x76, 0xff)
    };
    let button = if is_dark {
        (0xf6, 0xc4, 0x5d, 0xff)
    } else {
        (0x2d, 0x7d, 0xd2, 0xff)
    };
    let button_hover = if is_dark {
        (0xff, 0xd6, 0x7a, 0xff)
    } else {
        (0x22, 0x68, 0xb0, 0xff)
    };
    let button_pressed = if is_dark {
        (0xd8, 0x9d, 0x31, 0xff)
    } else {
        (0x1b, 0x4f, 0x88, 0xff)
    };
    let button_text = if is_dark {
        (0x1a, 0x16, 0x0d, 0xff)
    } else {
        (0xff, 0xff, 0xff, 0xff)
    };
    let toggle_theme = {
        let motherboard = motherboard.clone();
        let next_theme = next_theme.to_string();

        move || {
            let _ = motherboard.client().calls().call(
                SERVICE_NAME,
                "setTheme",
                next_theme.as_bytes().to_vec(),
                Box::<[u32]>::default(),
            );
        }
    };

    rsml! {
        <container
            w_expand
            h_expand
            center
            background_color={background}
            direction={Direction::Column}
            gap={18}
            padding_all={28}
        >
            <container
                direction={Direction::Column}
                gap={12}
                padding_all={24}
                rounded={8.0}
                background_color={panel}
                border_width={1}
                border_color={if is_dark {
                    (0x34, 0x3f, 0x4d, 0xff)
                } else {
                    (0xd8, 0xde, 0xe6, 0xff)
                }}
                min_width={320.0}
            >
                <text
                    font_size={22}
                    color={text}
                    font_family="UbuntuSans NF"
                >
                    Settings
                </text>

                <text
                    font_size={14}
                    color={secondary_text}
                    font_family="UbuntuSans NF"
                >
                    {format!("Theme store: {theme}")}
                </text>

                <container
                    w_fit
                    padding_all={14}
                    rounded={8.0}
                    background_color={button}
                    on_click={toggle_theme}
                    style_if_hovered={move |style| ContainerStyle {
                        background_color: button_hover.into(),
                        ..style
                    }}
                    style_if_pressed={move |style| ContainerStyle {
                        background_color: button_pressed.into(),
                        ..style
                    }}
                    focusable
                >
                    <text
                        font_size={15}
                        color={button_text}
                        font_family="UbuntuSans NF"
                        text_center
                    >
                        {format!("Switch to {next_theme}")}
                    </text>
                </container>
            </container>
        </container>
    }
}

fn main() {
    env_logger::init();

    ardos_ui::create_window_winit(
        settings_app,
        WindowOptions {
            title: "Settings".into(),
            preferred_size: (420.0, 280.0),
            min_size: (360.0, 240.0),
            opaque: true,
            ..Default::default()
        },
    );
}
