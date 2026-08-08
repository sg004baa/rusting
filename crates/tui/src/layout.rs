use ratatui::layout::Rect;
use rusting_core::config::{Settings, SidebarPosition};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frames {
    pub header: Option<Rect>,
    pub sidebar: Option<Rect>,
    pub url_bar: Rect,
    pub request: Option<Rect>,
    pub response: Option<Rect>,
    pub footer: Rect,
}

pub fn compute(
    screen: Rect,
    settings: &Settings,
    sidebar_visible: bool,
    expanded: Option<Section>,
    show_value_preview: bool,
) -> Frames {
    let header_height = if settings.heading.visible {
        3.min(screen.height)
    } else {
        0
    };
    let footer_height = u16::from(screen.height > header_height);
    let body_y = screen.y.saturating_add(header_height);
    let body_height = screen
        .height
        .saturating_sub(header_height)
        .saturating_sub(footer_height);

    let header = settings.heading.visible.then(|| {
        inset_horizontal(
            Rect::new(screen.x, screen.y, screen.width, header_height),
            3,
        )
    });
    let footer = inset_left(
        Rect::new(
            screen.x,
            screen
                .y
                .saturating_add(screen.height.saturating_sub(footer_height)),
            screen.width,
            footer_height,
        ),
        2,
    );
    let body = inset_horizontal(Rect::new(screen.x, body_y, screen.width, body_height), 2);

    let (sidebar, main) = if sidebar_visible && body.width >= 3 {
        let sidebar_width = ((u32::from(body.width) * 25) / 100) as u16;
        let sidebar_width = sidebar_width.max(24).min(body.width.saturating_sub(2));
        let main_width = body.width.saturating_sub(sidebar_width).saturating_sub(1);
        match settings.collection_browser.position {
            SidebarPosition::Left => (
                Some(Rect::new(body.x, body.y, sidebar_width, body.height)),
                Rect::new(
                    body.x.saturating_add(sidebar_width).saturating_add(1),
                    body.y,
                    main_width,
                    body.height,
                ),
            ),
            SidebarPosition::Right => (
                Some(Rect::new(
                    body.x.saturating_add(main_width).saturating_add(1),
                    body.y,
                    sidebar_width,
                    body.height,
                )),
                Rect::new(body.x, body.y, main_width, body.height),
            ),
        }
    } else {
        (None, body)
    };

    let url_height = if show_value_preview { 4 } else { 3 }.min(main.height);
    let url_bar = Rect::new(main.x, main.y, main.width, url_height);
    let panes = Rect::new(
        main.x,
        main.y.saturating_add(url_height),
        main.width,
        main.height.saturating_sub(url_height),
    );
    let (request, response) = match expanded {
        Some(Section::Request) => (Some(panes), None),
        Some(Section::Response) => (None, Some(panes)),
        None => {
            let request_height = panes.height / 2;
            let response_height = panes.height.saturating_sub(request_height);
            (
                Some(Rect::new(panes.x, panes.y, panes.width, request_height)),
                Some(Rect::new(
                    panes.x,
                    panes.y.saturating_add(request_height),
                    panes.width,
                    response_height,
                )),
            )
        }
    };

    Frames {
        header,
        sidebar,
        url_bar,
        request,
        response,
        footer,
    }
}

fn inset_horizontal(area: Rect, amount: u16) -> Rect {
    let inset = amount.min(area.width / 2);
    Rect::new(
        area.x.saturating_add(inset),
        area.y,
        area.width.saturating_sub(inset.saturating_mul(2)),
        area.height,
    )
}

fn inset_left(area: Rect, amount: u16) -> Rect {
    let inset = amount.min(area.width);
    Rect::new(
        area.x.saturating_add(inset),
        area.y,
        area.width.saturating_sub(inset),
        area.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_layout_respects_dock_and_equal_split() {
        let mut settings = Settings::default();
        settings.collection_browser.position = SidebarPosition::Right;
        let frames = compute(Rect::new(0, 0, 120, 40), &settings, true, None, true);
        let sidebar = frames.sidebar.unwrap();
        let request = frames.request.unwrap();
        let response = frames.response.unwrap();
        assert!(sidebar.x > frames.url_bar.x);
        assert_eq!(frames.url_bar.height, 4);
        assert!((i32::from(request.height) - i32::from(response.height)).abs() <= 1);
        assert_eq!(frames.header.unwrap().x, 3);
        assert_eq!(frames.footer.height, 1);
    }

    #[test]
    fn expansion_removes_the_other_section() {
        let settings = Settings::default();
        let frames = compute(
            Rect::new(0, 0, 80, 24),
            &settings,
            false,
            Some(Section::Response),
            false,
        );
        assert!(frames.request.is_none());
        assert!(frames.response.is_some());
        assert_eq!(frames.url_bar.height, 3);
    }
}
