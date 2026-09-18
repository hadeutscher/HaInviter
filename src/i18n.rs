//! Localisation.
//!
//! One locale per deployment, chosen by `HAINVITER_LOCALE`. The server resolves
//! it from the environment and hands it to the client through a server function,
//! so server-rendered markup and the hydrated client agree on every string —
//! which they must, or hydration mismatches.
//!
//! Strings live in one struct per language rather than in a lookup map: a missing
//! translation is then a compile error instead of a runtime hole.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Locales
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Locale {
    /// English, left to right.
    #[default]
    EnUs,
    /// Hebrew, right to left.
    HeIl,
}

impl Locale {
    /// Parses a BCP 47 tag. Anything unrecognised falls back to `en-US` rather
    /// than failing: a typo in a deployment variable must not take the site down.
    ///
    /// Only the server reads the environment; the client is told the answer.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub fn from_tag(tag: &str) -> Self {
        let tag = tag.trim().to_ascii_lowercase();
        let language = tag.split(['-', '_']).next().unwrap_or_default();
        match language {
            // "iw" is the deprecated code for Hebrew and still turns up.
            "he" | "iw" => Self::HeIl,
            _ => Self::EnUs,
        }
    }

    /// The BCP 47 tag, for logs and the audit trail — both server-side.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::HeIl => "he-IL",
        }
    }

    /// Value for the `lang` attribute.
    pub const fn lang(self) -> &'static str {
        match self {
            Self::EnUs => "en",
            Self::HeIl => "he",
        }
    }

    /// Value for the `dir` attribute. The stylesheet keys its right-to-left
    /// adjustments off this.
    pub const fn dir(self) -> &'static str {
        match self {
            Self::EnUs => "ltr",
            Self::HeIl => "rtl",
        }
    }

    pub const fn strings(self) -> &'static Strings {
        match self {
            Self::EnUs => &EN,
            Self::HeIl => &HE,
        }
    }

    // ── Dates ──────────────────────────────────────────────────────────────
    //
    // chrono only formats month and weekday names in English, so the names come
    // from the string tables and the assembly happens here.

    /// Formats a `YYYY-MM-DDTHH:MM` local timestamp in full, e.g.
    /// `"Saturday, 12 September 2026 at 19:30"`.
    ///
    /// Unparseable input is returned unchanged rather than hidden, so a typo in
    /// the admin panel is visible instead of silently blanking the invitation.
    pub fn format_datetime(self, raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let s = self.strings();
        for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, fmt) {
                return s
                    .datetime_pattern
                    .replace("{date}", &self.format_naive_date(dt.date()))
                    .replace("{time}", &dt.format("%H:%M").to_string());
            }
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
            return self.format_naive_date(d);
        }
        trimmed.to_owned()
    }

    /// Formats a `YYYY-MM-DD` date without the weekday, e.g. `"12 September 2026"`.
    pub fn format_date(self, raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        match chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
            Ok(d) => self.format_day(d),
            Err(_) => trimmed.to_owned(),
        }
    }

    /// `"Saturday, 12 September 2026"` — weekday included.
    fn format_naive_date(self, d: chrono::NaiveDate) -> String {
        use chrono::Datelike;
        let s = self.strings();
        s.date_pattern
            .replace(
                "{weekday}",
                s.weekdays[d.weekday().num_days_from_sunday() as usize],
            )
            .replace(
                "{day}",
                &s.day_and_month.replace("{day}", &d.day().to_string()),
            )
            .replace("{month}", s.months[(d.month0()) as usize])
            .replace("{year}", &d.year().to_string())
    }

    /// `"12 September 2026"` — no weekday.
    fn format_day(self, d: chrono::NaiveDate) -> String {
        use chrono::Datelike;
        let s = self.strings();
        s.day_pattern
            .replace("{day}", &d.day().to_string())
            .replace("{month}", s.months[(d.month0()) as usize])
            .replace("{year}", &d.year().to_string())
    }

    // ── Counted strings ────────────────────────────────────────────────────

    pub fn n_of_us(self, n: i64) -> String {
        if n <= 1 {
            self.strings().just_me.to_owned()
        } else {
            self.strings().n_of_us.replace("{n}", &n.to_string())
        }
    }

    pub fn may_bring(self, n: i64) -> String {
        if n <= 1 {
            self.strings().just_themselves.to_owned()
        } else {
            self.strings().n_people.replace("{n}", &n.to_string())
        }
    }

    /// The label for an RSVP state.
    pub fn status(self, status: crate::types::Rsvp) -> &'static str {
        let s = self.strings();
        match status {
            crate::types::Rsvp::Pending => s.status_pending,
            crate::types::Rsvp::Attending => s.status_attending,
            crate::types::Rsvp::Declined => s.status_declined,
        }
    }
}

/// The locale this process serves, from `HAINVITER_LOCALE`.
#[cfg(feature = "server")]
pub fn from_env() -> Locale {
    static LOCALE: std::sync::OnceLock<Locale> = std::sync::OnceLock::new();
    *LOCALE.get_or_init(|| {
        std::env::var("HAINVITER_LOCALE")
            .map(|t| Locale::from_tag(&t))
            .unwrap_or_default()
    })
}

/// The locale in effect for the component being rendered.
///
/// Provided as context by the root component; falls back to the default so a
/// component rendered outside it (a test, a preview) still works.
pub fn active() -> Locale {
    dioxus::prelude::try_consume_context::<Locale>().unwrap_or_default()
}

/// Shorthand for the active locale's strings.
pub fn t() -> &'static Strings {
    active().strings()
}

// ---------------------------------------------------------------------------
// The string table
// ---------------------------------------------------------------------------

/// Every piece of user-visible text. `{}`-style placeholders are filled by the
/// caller; the comment on each notes what goes in.
///
/// The `err_*` messages are produced by server functions and so are dead weight
/// in the WASM build, which still has to carry the struct definition.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub struct Strings {
    // ── Dates ──
    /// `{weekday}`, `{day}`, `{month}`, `{year}`
    pub date_pattern: &'static str,
    /// `{day}`, `{month}`, `{year}`
    pub day_pattern: &'static str,
    /// Lets Hebrew attach its month preposition: `{day}`
    pub day_and_month: &'static str,
    /// `{date}`, `{time}`
    pub datetime_pattern: &'static str,
    pub weekdays: [&'static str; 7],
    pub months: [&'static str; 12],

    // ── Shared ──
    pub app_name: &'static str,
    pub cancel: &'static str,
    pub save: &'static str,
    pub saving: &'static str,
    pub loading: &'static str,
    pub reload: &'static str,

    // ── RSVP states ──
    pub status_pending: &'static str,
    pub status_attending: &'static str,
    pub status_declined: &'static str,

    // ── Landing / not found ──
    pub home_body: &'static str,
    pub home_note: &'static str,
    pub not_found_title: &'static str,
    /// `{}` is the path that was requested
    pub not_found_body: &'static str,
    pub not_found_note: &'static str,

    // ── Invitation ──
    pub invite_loading: &'static str,
    pub invite_invalid_title: &'static str,
    pub invite_invalid_body: &'static str,
    pub invite_invalid_note: &'static str,
    pub invite_page_title_suffix: &'static str,
    /// `{}` is the guest's name
    pub invite_greeting: &'static str,
    pub invite_default_body: &'static str,
    pub invite_when: &'static str,
    pub invite_where: &'static str,
    pub invite_open_maps: &'static str,
    pub invite_question: &'static str,
    /// `{}` is the deadline date
    pub invite_reply_by: &'static str,
    pub invite_closed: &'static str,
    /// `{}` is the recorded answer
    pub invite_closed_answered: &'static str,
    pub invite_closed_contact: &'static str,
    pub invite_yes: &'static str,
    pub invite_no: &'static str,
    pub invite_party_question: &'static str,
    pub just_me: &'static str,
    /// `{n}` people including the guest
    pub n_of_us: &'static str,
    pub invite_note_label: &'static str,
    pub invite_note_help: &'static str,
    pub invite_choose_first: &'static str,
    pub invite_thanks_yes: &'static str,
    pub invite_thanks_no: &'static str,
    pub invite_send: &'static str,
    pub invite_update: &'static str,
    pub invite_sending: &'static str,
    pub invite_footer: &'static str,

    // ── Admin: shell ──
    pub admin_events: &'static str,
    pub admin_subtitle: &'static str,
    pub admin_link_warning: &'static str,
    pub admin_back: &'static str,
    pub admin_invalid_title: &'static str,
    pub admin_invalid_body: &'static str,
    pub admin_invalid_note_before: &'static str,
    pub admin_invalid_note_after: &'static str,
    pub admin_loading_events: &'static str,

    // ── Admin: event list ──
    pub admin_no_events_title: &'static str,
    pub admin_no_events_body: &'static str,
    pub admin_new_event: &'static str,
    pub admin_create_event: &'static str,
    pub admin_creating: &'static str,
    pub admin_needs_title: &'static str,
    /// `{invited}`, `{expected}`
    pub admin_invited_expected: &'static str,
    /// `{n}`
    pub admin_n_coming: &'static str,
    /// `{n}`
    pub admin_n_declined: &'static str,
    /// `{n}`
    pub admin_n_waiting: &'static str,

    // ── Admin: tabs ──
    pub tab_details: &'static str,
    /// `{n}`
    pub tab_guests: &'static str,
    pub tab_responses: &'static str,

    // ── Admin: event form ──
    pub field_title: &'static str,
    pub field_title_help: &'static str,
    pub field_hosts: &'static str,
    pub field_hosts_help: &'static str,
    pub field_starts: &'static str,
    pub field_location: &'static str,
    pub field_map: &'static str,
    pub field_map_help: &'static str,
    pub field_deadline: &'static str,
    pub field_deadline_help: &'static str,
    pub field_body: &'static str,
    pub field_body_help: &'static str,
    pub field_cover: &'static str,
    pub field_cover_upload: &'static str,
    pub field_cover_uploading: &'static str,
    pub field_cover_remove: &'static str,
    pub field_cover_url: &'static str,
    pub field_cover_preview_alt: &'static str,
    pub field_cover_unreadable: &'static str,
    /// `{}` is the size limit in MB
    pub field_cover_too_large: &'static str,
    pub field_plus_ones: &'static str,
    pub admin_save_changes: &'static str,
    pub admin_saved: &'static str,
    pub admin_delete_event: &'static str,
    pub admin_delete_event_title: &'static str,
    pub admin_delete_event_body: &'static str,
    pub admin_keep_it: &'static str,

    // ── Admin: guests ──
    pub guests_add_title: &'static str,
    pub guests_one_per_line: &'static str,
    pub guests_paste_help: &'static str,
    pub guests_default_party: &'static str,
    pub guests_add_button: &'static str,
    pub guests_adding: &'static str,
    pub guests_type_a_name: &'static str,
    /// `{n}`
    pub guests_added_n: &'static str,
    pub guests_all_present: &'static str,
    pub guests_list_title: &'static str,
    /// `{n}`
    pub guests_n_invited: &'static str,
    pub guests_none_title: &'static str,
    pub guests_none_body: &'static str,
    pub col_guest: &'static str,
    pub col_reply: &'static str,
    pub col_party: &'static str,
    pub col_link: &'static str,
    pub col_note: &'static str,
    pub col_when: &'static str,
    /// `{}` is a timestamp
    pub guests_replied_at: &'static str,
    pub guests_edit_title: &'static str,
    pub guests_name: &'static str,
    pub guests_may_bring: &'static str,
    pub just_themselves: &'static str,
    /// `{n}`
    pub n_people: &'static str,
    pub guests_remove_title: &'static str,
    /// `{}` is the guest's name
    pub guests_remove_body: &'static str,
    pub guests_remove_button: &'static str,
    /// `{}` is the guest's name
    pub guests_removed: &'static str,
    /// `{}` is the guest's name
    pub guests_link_reissued: &'static str,
    /// `{}` is the guest's name
    pub guests_reply_cleared: &'static str,
    pub action_copy_link: &'static str,
    pub action_new_link: &'static str,
    pub action_clear_reply: &'static str,
    pub action_edit_guest: &'static str,
    pub action_remove_guest: &'static str,

    // ── Admin: responses ──
    pub stat_invited: &'static str,
    pub stat_coming: &'static str,
    pub stat_declined: &'static str,
    pub stat_waiting: &'static str,
    pub stat_expected: &'static str,
    pub responses_download: &'static str,
    pub responses_title: &'static str,
    pub responses_none_title: &'static str,
    pub responses_none_body: &'static str,

    // ── Server-side messages ──
    pub err_not_authorised: &'static str,
    pub err_event_needs_title: &'static str,
    pub err_no_such_event: &'static str,
    pub err_no_such_guest: &'static str,
    pub err_no_names: &'static str,
    pub err_guest_needs_name: &'static str,
    pub err_invite_not_found: &'static str,
    pub err_replies_closed: &'static str,
    pub err_file_empty: &'static str,
    pub err_not_an_image: &'static str,
    pub err_store_failed: &'static str,
}

pub static EN: Strings = Strings {
    date_pattern: "{weekday}, {day} {month} {year}",
    day_pattern: "{day} {month} {year}",
    day_and_month: "{day}",
    datetime_pattern: "{date} at {time}",
    weekdays: [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ],
    months: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],

    app_name: "HaInviter",
    cancel: "Cancel",
    save: "Save",
    saving: "Saving…",
    loading: "Loading…",
    reload: "Reload",

    status_pending: "Awaiting reply",
    status_attending: "Attending",
    status_declined: "Not attending",

    home_body: "Invitations here are personal. Open the link the hosts sent you to see your \
                invitation and let them know whether you can come.",
    home_note: "Lost your link? Ask whoever invited you to send it again.",
    not_found_title: "Nothing here",
    not_found_body: "We could not find {}.",
    not_found_note: "Invitation links look like /i/… — check the link you were sent, or ask the \
                     hosts to resend it.",

    invite_loading: "Opening your invitation…",
    invite_invalid_title: "This invitation link is not valid",
    invite_invalid_body: "The link may have been copied incompletely, or replaced with a new one.",
    invite_invalid_note: "Please ask the hosts to send you a fresh link.",
    invite_page_title_suffix: "invitation for",
    invite_greeting: "Dear {},",
    invite_default_body: "You are invited — we would love to see you there.",
    invite_when: "When",
    invite_where: "Where",
    invite_open_maps: "Open in maps",
    invite_question: "Will you join us?",
    invite_reply_by: "Please reply by {}.",
    invite_closed: "Replies have closed.",
    invite_closed_answered: "Replies have closed. Your answer is recorded as “{}”.",
    invite_closed_contact: "If something has changed, please contact the hosts directly.",
    invite_yes: "Yes, I'll be there",
    invite_no: "Sorry, I can't",
    invite_party_question: "How many of you are coming?",
    just_me: "Just me",
    n_of_us: "{n} of us",
    invite_note_label: "A note for the hosts (optional)",
    invite_note_help: "Dietary needs, a song request, or just hello.",
    invite_choose_first: "Please choose whether you can come.",
    invite_thanks_yes: "Thank you — we have you down. See you there!",
    invite_thanks_no: "Thank you for letting us know — you will be missed.",
    invite_send: "Send my reply",
    invite_update: "Update my reply",
    invite_sending: "Sending…",
    invite_footer: "This invitation is personal to you — please do not forward the link.",

    admin_events: "Events",
    admin_subtitle: "HaInviter admin",
    admin_link_warning: "Anyone with this page's address can administer HaInviter. Keep it out of \
                         chats and screenshots.",
    admin_back: "Back to events",
    admin_invalid_title: "This admin link is not valid",
    admin_invalid_body: "The admin link is printed to the server log when HaInviter starts.",
    admin_invalid_note_before: "Run ",
    admin_invalid_note_after: " to read it, or set HAINVITER_ADMIN_TOKEN to keep the link stable.",
    admin_loading_events: "Loading events…",

    admin_no_events_title: "No events yet",
    admin_no_events_body: "Create an event, then add the people you want to invite. Each guest \
                           gets their own private link.",
    admin_new_event: "New event",
    admin_create_event: "Create event",
    admin_creating: "Creating…",
    admin_needs_title: "An event needs a title.",
    admin_invited_expected: "{invited} invited · {expected} expected",
    admin_n_coming: "{n} coming",
    admin_n_declined: "{n} declined",
    admin_n_waiting: "{n} waiting",

    tab_details: "Details",
    tab_guests: "Guests ({n})",
    tab_responses: "Responses",

    field_title: "Event title",
    field_title_help: "Shown as the invitation's headline.",
    field_hosts: "Hosted by",
    field_hosts_help: "e.g. Dana & Yuval",
    field_starts: "Starts",
    field_location: "Location",
    field_map: "Map link",
    field_map_help: "Optional — opens in the guest's maps app.",
    field_deadline: "Reply by",
    field_deadline_help: "After this date the form becomes read-only.",
    field_body: "Invitation text",
    field_body_help: "Leave a blank line between paragraphs.",
    field_cover: "Cover image",
    field_cover_upload: "Upload image",
    field_cover_uploading: "Uploading…",
    field_cover_remove: "Remove",
    field_cover_url: "…or paste an image URL",
    field_cover_preview_alt: "Cover preview",
    field_cover_unreadable: "Could not read that file.",
    field_cover_too_large: "That image is larger than {} MB.",
    field_plus_ones: "Guests may bring the people on their invitation",
    admin_save_changes: "Save changes",
    admin_saved: "Event saved.",
    admin_delete_event: "Delete event",
    admin_delete_event_title: "Delete this event?",
    admin_delete_event_body: "The event and its guest list will be removed, and every invitation \
                              link for it will stop working. Replies already received stay in the \
                              audit log and in the reply history, but they will no longer be \
                              listed here or exported.",
    admin_keep_it: "Keep it",

    guests_add_title: "Add guests",
    guests_one_per_line: "One name per line",
    guests_paste_help: "Add “, 4” after a name to let that guest bring up to four people. Names \
                        already invited are skipped.",
    guests_default_party: "Default party size",
    guests_add_button: "Add to guest list",
    guests_adding: "Adding…",
    guests_type_a_name: "Type at least one name.",
    guests_added_n: "Added {n} guests.",
    guests_all_present: "Everyone on that list was already invited.",
    guests_list_title: "Guest list",
    guests_n_invited: "{n} invited",
    guests_none_title: "Nobody invited yet",
    guests_none_body: "Paste your guest list above. Each name gets a private invitation link.",
    col_guest: "Guest",
    col_reply: "Reply",
    col_party: "Party",
    col_link: "Invitation link",
    col_note: "Note",
    col_when: "When",
    guests_replied_at: "replied {}",
    guests_edit_title: "Edit guest",
    guests_name: "Name",
    guests_may_bring: "May bring up to",
    just_themselves: "just themselves",
    n_people: "{n} people",
    guests_remove_title: "Remove this guest?",
    guests_remove_body: "{} will be removed and their invitation link will stop working. Any \
                         reply they already sent stays in the audit log.",
    guests_remove_button: "Remove guest",
    guests_removed: "Removed {}.",
    guests_link_reissued: "New link issued for {}.",
    guests_reply_cleared: "Cleared the reply from {}.",
    action_copy_link: "Copy invitation link",
    action_new_link: "Issue a new link",
    action_clear_reply: "Clear this reply",
    action_edit_guest: "Edit guest",
    action_remove_guest: "Remove guest",

    stat_invited: "Invited",
    stat_coming: "Coming",
    stat_declined: "Declined",
    stat_waiting: "Waiting",
    stat_expected: "People expected",
    responses_download: "Download CSV",
    responses_title: "Replies",
    responses_none_title: "No replies yet",
    responses_none_body: "Replies appear here as guests open their links. Every one is also \
                          written to the audit log.",

    err_not_authorised: "not authorised",
    err_event_needs_title: "an event needs a title",
    err_no_such_event: "no such event",
    err_no_such_guest: "no such guest",
    err_no_names: "no names found in that list",
    err_guest_needs_name: "a guest needs a name",
    err_invite_not_found: "invitation not found",
    err_replies_closed: "replies for this event have closed — please contact the hosts directly",
    err_file_empty: "that file is empty",
    err_not_an_image: "that does not look like a JPEG, PNG, GIF or WebP",
    err_store_failed: "could not store the image",
};

pub static HE: Strings = Strings {
    // Hebrew writes "12 בספטמבר 2026": the month takes a ב prefix, and the
    // weekday is "יום" plus the ordinal.
    date_pattern: "{weekday}, {day} ב{month} {year}",
    day_pattern: "{day} ב{month} {year}",
    day_and_month: "{day}",
    datetime_pattern: "{date} בשעה {time}",
    weekdays: [
        "יום ראשון",
        "יום שני",
        "יום שלישי",
        "יום רביעי",
        "יום חמישי",
        "יום שישי",
        "יום שבת",
    ],
    months: [
        "ינואר",
        "פברואר",
        "מרץ",
        "אפריל",
        "מאי",
        "יוני",
        "יולי",
        "אוגוסט",
        "ספטמבר",
        "אוקטובר",
        "נובמבר",
        "דצמבר",
    ],

    app_name: "המזמין",
    cancel: "ביטול",
    save: "שמירה",
    saving: "שומר…",
    loading: "טוען…",
    reload: "רענון",

    status_pending: "ממתין לתשובה",
    status_attending: "מגיע",
    status_declined: "לא מגיע",

    home_body: "ההזמנות כאן אישיות. פתחו את הקישור שקיבלתם מהמזמינים כדי לראות את ההזמנה ולעדכן \
                אם תוכלו להגיע.",
    home_note: "אבד לכם הקישור? בקשו ממי שהזמין אתכם לשלוח אותו שוב.",
    not_found_title: "אין כאן כלום",
    not_found_body: "לא מצאנו את {}.",
    not_found_note: "קישורי הזמנה נראים כמו /i/… — בדקו את הקישור שקיבלתם, או בקשו מהמזמינים \
                     לשלוח אותו מחדש.",

    invite_loading: "פותח את ההזמנה שלכם…",
    invite_invalid_title: "קישור ההזמנה הזה אינו תקף",
    invite_invalid_body: "ייתכן שהקישור הועתק חלקית, או שהוחלף בקישור חדש.",
    invite_invalid_note: "בקשו מהמזמינים לשלוח לכם קישור חדש.",
    invite_page_title_suffix: "הזמנה עבור",
    invite_greeting: "{} שלום,",
    invite_default_body: "אתם מוזמנים — נשמח מאוד לראותכם.",
    invite_when: "מתי",
    invite_where: "איפה",
    invite_open_maps: "פתיחה במפות",
    invite_question: "תוכלו להגיע?",
    invite_reply_by: "נא להשיב עד {}.",
    invite_closed: "ההרשמה נסגרה.",
    invite_closed_answered: "ההרשמה נסגרה. התשובה שלכם רשומה כ„{}”.",
    invite_closed_contact: "אם משהו השתנה, פנו ישירות למזמינים.",
    invite_yes: "כן, אגיע",
    invite_no: "מצטער, לא אוכל",
    invite_party_question: "כמה אנשים תגיעו?",
    just_me: "רק אני",
    n_of_us: "{n} אנשים",
    invite_note_label: "הערה למזמינים (לא חובה)",
    invite_note_help: "העדפות תזונה, בקשת שיר, או סתם שלום.",
    invite_choose_first: "בחרו אם תוכלו להגיע.",
    invite_thanks_yes: "תודה — רשמנו אתכם. נתראה!",
    invite_thanks_no: "תודה על העדכון — תחסרו לנו.",
    invite_send: "שליחת התשובה",
    invite_update: "עדכון התשובה",
    invite_sending: "שולח…",
    invite_footer: "ההזמנה הזו אישית לכם — אנא אל תעבירו את הקישור.",

    admin_events: "אירועים",
    admin_subtitle: "ניהול המזמין",
    admin_link_warning: "כל מי שיש לו את הכתובת של הדף הזה יכול לנהל את המזמין. שמרו אותה מחוץ \
                         לצ׳אטים ולצילומי מסך.",
    admin_back: "חזרה לאירועים",
    admin_invalid_title: "קישור הניהול הזה אינו תקף",
    admin_invalid_body: "קישור הניהול מודפס ליומן השרת בכל הפעלה של המזמין.",
    admin_invalid_note_before: "הריצו ",
    admin_invalid_note_after: " כדי לקרוא אותו, או הגדירו HAINVITER_ADMIN_TOKEN כדי לקבע את הקישור.",
    admin_loading_events: "טוען אירועים…",

    admin_no_events_title: "אין עוד אירועים",
    admin_no_events_body: "צרו אירוע, ואז הוסיפו את האנשים שתרצו להזמין. כל מוזמן מקבל קישור \
                           פרטי משלו.",
    admin_new_event: "אירוע חדש",
    admin_create_event: "יצירת אירוע",
    admin_creating: "יוצר…",
    admin_needs_title: "לאירוע דרושה כותרת.",
    admin_invited_expected: "{invited} מוזמנים · {expected} צפויים",
    admin_n_coming: "{n} מגיעים",
    admin_n_declined: "{n} לא מגיעים",
    admin_n_waiting: "{n} ממתינים",

    tab_details: "פרטים",
    tab_guests: "מוזמנים ({n})",
    tab_responses: "תשובות",

    field_title: "כותרת האירוע",
    field_title_help: "מוצגת ככותרת ההזמנה.",
    field_hosts: "בהזמנת",
    field_hosts_help: "לדוגמה: דנה ויובל",
    field_starts: "מתחיל",
    field_location: "מקום",
    field_map: "קישור למפה",
    field_map_help: "לא חובה — נפתח באפליקציית המפות של המוזמן.",
    field_deadline: "תשובה עד",
    field_deadline_help: "לאחר התאריך הזה הטופס נעשה לקריאה בלבד.",
    field_body: "נוסח ההזמנה",
    field_body_help: "השאירו שורה ריקה בין פסקאות.",
    field_cover: "תמונת רקע",
    field_cover_upload: "העלאת תמונה",
    field_cover_uploading: "מעלה…",
    field_cover_remove: "הסרה",
    field_cover_url: "…או הדביקו כתובת של תמונה",
    field_cover_preview_alt: "תצוגה מקדימה של תמונת הרקע",
    field_cover_unreadable: "לא הצלחנו לקרוא את הקובץ.",
    field_cover_too_large: "התמונה גדולה מ־{} מ״ב.",
    field_plus_ones: "מוזמנים יכולים להביא את מי שרשום בהזמנה שלהם",
    admin_save_changes: "שמירת השינויים",
    admin_saved: "האירוע נשמר.",
    admin_delete_event: "מחיקת האירוע",
    admin_delete_event_title: "למחוק את האירוע?",
    admin_delete_event_body: "האירוע ורשימת המוזמנים שלו יוסרו, וכל קישורי ההזמנה שלו יפסיקו \
                              לעבוד. תשובות שהתקבלו יישארו ביומן הביקורת ובהיסטוריית התשובות, \
                              אבל לא יופיעו כאן ולא ייוצאו לקובץ.",
    admin_keep_it: "להשאיר",

    guests_add_title: "הוספת מוזמנים",
    guests_one_per_line: "שם אחד בכל שורה",
    guests_paste_help: "הוסיפו „, 4” אחרי שם כדי לאפשר למוזמן להביא עד ארבעה אנשים. שמות שכבר \
                        הוזמנו יידלגו.",
    guests_default_party: "מספר אנשים כברירת מחדל",
    guests_add_button: "הוספה לרשימה",
    guests_adding: "מוסיף…",
    guests_type_a_name: "הקלידו שם אחד לפחות.",
    guests_added_n: "נוספו {n} מוזמנים.",
    guests_all_present: "כל מי שברשימה הזו כבר הוזמן.",
    guests_list_title: "רשימת המוזמנים",
    guests_n_invited: "{n} מוזמנים",
    guests_none_title: "עוד לא הוזמן אף אחד",
    guests_none_body: "הדביקו כאן את רשימת המוזמנים. כל שם מקבל קישור הזמנה פרטי.",
    col_guest: "מוזמן",
    col_reply: "תשובה",
    col_party: "אנשים",
    col_link: "קישור ההזמנה",
    col_note: "הערה",
    col_when: "מתי",
    guests_replied_at: "השיב {}",
    guests_edit_title: "עריכת מוזמן",
    guests_name: "שם",
    guests_may_bring: "יכול להביא עד",
    just_themselves: "רק את עצמו",
    n_people: "{n} אנשים",
    guests_remove_title: "להסיר את המוזמן?",
    guests_remove_body: "{} יוסר וקישור ההזמנה שלו יפסיק לעבוד. תשובה שכבר נשלחה תישאר ביומן \
                         הביקורת.",
    guests_remove_button: "הסרת המוזמן",
    guests_removed: "{} הוסר.",
    guests_link_reissued: "הונפק קישור חדש עבור {}.",
    guests_reply_cleared: "התשובה של {} נמחקה.",
    action_copy_link: "העתקת קישור ההזמנה",
    action_new_link: "הנפקת קישור חדש",
    action_clear_reply: "מחיקת התשובה",
    action_edit_guest: "עריכת המוזמן",
    action_remove_guest: "הסרת המוזמן",

    stat_invited: "מוזמנים",
    stat_coming: "מגיעים",
    stat_declined: "לא מגיעים",
    stat_waiting: "ממתינים",
    stat_expected: "אנשים צפויים",
    responses_download: "הורדת CSV",
    responses_title: "תשובות",
    responses_none_title: "עוד אין תשובות",
    responses_none_body: "התשובות יופיעו כאן כשהמוזמנים יפתחו את הקישורים שלהם. כל תשובה נרשמת \
                          גם ביומן הביקורת.",

    err_not_authorised: "אין הרשאה",
    err_event_needs_title: "לאירוע דרושה כותרת",
    err_no_such_event: "אירוע כזה לא קיים",
    err_no_such_guest: "מוזמן כזה לא קיים",
    err_no_names: "לא נמצאו שמות ברשימה",
    err_guest_needs_name: "למוזמן דרוש שם",
    err_invite_not_found: "ההזמנה לא נמצאה",
    err_replies_closed: "ההרשמה לאירוע הזה נסגרה — אנא פנו ישירות למזמינים",
    err_file_empty: "הקובץ ריק",
    err_not_an_image: "זה לא נראה כמו JPEG, PNG, GIF או WebP",
    err_store_failed: "לא הצלחנו לשמור את התמונה",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_resolve_to_locales() {
        assert_eq!(Locale::from_tag("he-IL"), Locale::HeIl);
        assert_eq!(Locale::from_tag("he"), Locale::HeIl);
        assert_eq!(Locale::from_tag("iw_IL"), Locale::HeIl);
        assert_eq!(Locale::from_tag("en-US"), Locale::EnUs);
        // An unknown or misspelled tag must not take the site down.
        assert_eq!(Locale::from_tag("klingon"), Locale::EnUs);
        assert_eq!(Locale::from_tag(""), Locale::EnUs);
    }

    #[test]
    fn direction_follows_the_locale() {
        assert_eq!(Locale::EnUs.dir(), "ltr");
        assert_eq!(Locale::HeIl.dir(), "rtl");
        assert_eq!(Locale::HeIl.lang(), "he");
    }

    #[test]
    fn dates_are_spelled_out_in_each_language() {
        assert_eq!(
            Locale::EnUs.format_datetime("2026-09-12T19:30"),
            "Saturday, 12 September 2026 at 19:30"
        );
        assert_eq!(
            Locale::HeIl.format_datetime("2026-09-12T19:30"),
            "יום שבת, 12 בספטמבר 2026 בשעה 19:30"
        );
        assert_eq!(Locale::EnUs.format_date("2026-09-12"), "12 September 2026");
        assert_eq!(Locale::HeIl.format_date("2026-09-12"), "12 בספטמבר 2026");
    }

    #[test]
    fn unparseable_dates_are_shown_as_typed() {
        assert_eq!(Locale::EnUs.format_datetime("  "), "");
        assert_eq!(Locale::EnUs.format_datetime("next spring"), "next spring");
        assert_eq!(Locale::HeIl.format_date("soon"), "soon");
    }

    #[test]
    fn counted_strings_switch_on_one() {
        assert_eq!(Locale::EnUs.n_of_us(1), "Just me");
        assert_eq!(Locale::EnUs.n_of_us(3), "3 of us");
        assert_eq!(Locale::HeIl.n_of_us(1), "רק אני");
        assert_eq!(Locale::EnUs.may_bring(1), "just themselves");
        assert_eq!(Locale::EnUs.may_bring(4), "4 people");
    }

    #[test]
    fn every_string_is_translated() {
        // A missing translation is a compile error, but an accidentally copied
        // English string in the Hebrew table is not — catch the obvious cases.
        assert_ne!(EN.invite_question, HE.invite_question);
        assert_ne!(EN.admin_events, HE.admin_events);
        assert_ne!(EN.err_invite_not_found, HE.err_invite_not_found);
        for (en, he) in EN.months.iter().zip(HE.months.iter()) {
            assert_ne!(en, he, "month name not translated");
        }
        for (en, he) in EN.weekdays.iter().zip(HE.weekdays.iter()) {
            assert_ne!(en, he, "weekday name not translated");
        }
    }
}
