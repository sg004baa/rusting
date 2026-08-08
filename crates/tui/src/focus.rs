use rusting_core::config::StartupFocus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Collection,
    Method,
    Url,
    RequestTabs,
    RequestBody,
    ResponseTabs,
    ResponseBody,
}

impl Focus {
    const WITH_COLLECTION: [Self; 7] = [
        Self::Collection,
        Self::Method,
        Self::Url,
        Self::RequestTabs,
        Self::RequestBody,
        Self::ResponseTabs,
        Self::ResponseBody,
    ];
    const WITHOUT_COLLECTION: [Self; 6] = [
        Self::Method,
        Self::Url,
        Self::RequestTabs,
        Self::RequestBody,
        Self::ResponseTabs,
        Self::ResponseBody,
    ];

    pub const fn from_startup(value: StartupFocus, sidebar_visible: bool) -> Self {
        match value {
            StartupFocus::Url => Self::Url,
            StartupFocus::Method => Self::Method,
            StartupFocus::Collection if sidebar_visible => Self::Collection,
            StartupFocus::Collection => Self::Url,
        }
    }

    pub fn next(self, sidebar_visible: bool) -> Self {
        self.advance(sidebar_visible, 1)
    }

    pub fn previous(self, sidebar_visible: bool) -> Self {
        self.advance(sidebar_visible, -1)
    }

    pub const fn request_section(self) -> bool {
        matches!(self, Self::RequestTabs | Self::RequestBody)
    }

    pub const fn response_section(self) -> bool {
        matches!(self, Self::ResponseTabs | Self::ResponseBody)
    }

    fn advance(self, sidebar_visible: bool, delta: isize) -> Self {
        let order = if sidebar_visible {
            Self::WITH_COLLECTION.as_slice()
        } else {
            Self::WITHOUT_COLLECTION.as_slice()
        };
        let current = order.iter().position(|candidate| *candidate == self);
        let index = match current {
            Some(index) => index,
            None if delta < 0 => 0,
            None => order.len() - 1,
        };
        let next = (index as isize + delta).rem_euclid(order.len() as isize) as usize;
        order[next]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_wraps_and_skips_hidden_collection() {
        assert_eq!(Focus::ResponseBody.next(true), Focus::Collection);
        assert_eq!(Focus::Method.previous(true), Focus::Collection);
        assert_eq!(Focus::ResponseBody.next(false), Focus::Method);
        assert_eq!(Focus::Method.previous(false), Focus::ResponseBody);
        assert_eq!(Focus::Collection.next(false), Focus::Method);
        assert_eq!(Focus::Collection.previous(false), Focus::ResponseBody);
    }
}
