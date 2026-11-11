//! # The class names of the view renderer, and the sheet that styles them
//!
//! A module declares no CSS: [`Look`](this_descriptor::views::Look) is a closed set of
//! choices (accent, layout, transition). The app therefore owns the sheet, and this crate
//! ships the one that goes with the renderer: [`CSS`]. Injecting it once is enough —
//! `document.head`, a `<style>` in the host page, or the app's own bundle.
//!
//! Every element the renderer draws carries a class from this module, never a literal
//! string, and two tests keep the two halves together: every constant appears in [`CSS`],
//! and [`CSS`] styles no `vw-` class that the module does not declare.

use this_descriptor::views::{Accent, Layout, Transition};

/// The classes of the view renderer. `ALL` mirrors them: keep both in sync.
pub mod class {
    pub const SURFACE: &str = "vw-surface";
    pub const SIDEBAR: &str = "vw-sidebar";
    pub const HEADER: &str = "vw-header";
    pub const MAIN: &str = "vw-main";
    pub const PANEL: &str = "vw-panel";

    pub const VIEW: &str = "vw-view";
    pub const VIEW_TITLE: &str = "vw-view-title";
    pub const VIEW_BODY: &str = "vw-view-body";
    pub const SR_ONLY: &str = "vw-sr-only";
    pub const STACK: &str = "vw-stack";
    pub const ROW: &str = "vw-row";
    pub const TABS: &str = "vw-tabs";
    pub const TABLIST: &str = "vw-tablist";
    pub const TAB: &str = "vw-tab";
    pub const TABPANEL: &str = "vw-tabpanel";
    pub const DRAWER: &str = "vw-drawer";
    pub const DRAWER_BODY: &str = "vw-drawer-body";
    pub const MODAL: &str = "vw-modal";
    pub const MODAL_HEAD: &str = "vw-modal-head";
    pub const MODAL_BODY: &str = "vw-modal-body";
    pub const MODAL_LAYER: &str = "vw-modal-layer";
    pub const MODAL_BACKDROP: &str = "vw-modal-backdrop";

    pub const TEXT: &str = "vw-text";
    pub const CAPTION: &str = "vw-caption";
    pub const CHART: &str = "vw-chart";
    pub const CHART_TITLE: &str = "vw-chart-title";
    pub const CHART_SCROLL: &str = "vw-chart-scroll";
    pub const BUTTON: &str = "vw-btn";
    pub const ICON_BUTTON: &str = "vw-icon-btn";
    pub const BANNER: &str = "vw-banner";
    pub const TONE_INFO: &str = "vw-tone-info";
    pub const TONE_SUCCESS: &str = "vw-tone-success";
    pub const TONE_WARNING: &str = "vw-tone-warning";
    pub const TONE_ERROR: &str = "vw-tone-error";
    pub const INDICATOR: &str = "vw-indicator";
    pub const DOT: &str = "vw-dot";
    pub const ACTIVITY: &str = "vw-activity";
    pub const METRICS: &str = "vw-metrics";

    pub const LIST: &str = "vw-list";
    pub const LIST_ROW: &str = "vw-list-row";
    pub const LIST_ROW_CLICK: &str = "vw-list-row-click";
    pub const LIST_ACTIONS: &str = "vw-list-actions";
    pub const LIST_LABEL: &str = "vw-list-label";
    pub const LIST_SUB: &str = "vw-list-sub";
    pub const LIST_TOOLS: &str = "vw-list-tools";
    pub const SEARCH: &str = "vw-search";
    pub const PAGER: &str = "vw-pager";
    pub const PAGER_INFO: &str = "vw-pager-info";

    pub const CARD: &str = "vw-card";
    pub const CARD_ROW: &str = "vw-card-row";
    pub const CARD_KEY: &str = "vw-card-key";
    pub const CARD_VALUE: &str = "vw-card-value";

    pub const FORM: &str = "vw-form";
    pub const FIELD: &str = "vw-field";
    pub const LABEL: &str = "vw-label";
    pub const INPUT: &str = "vw-input";
    pub const TEXTAREA: &str = "vw-textarea";
    pub const SELECT: &str = "vw-select";
    pub const TOGGLE: &str = "vw-toggle";
    pub const FILE: &str = "vw-file";
    pub const HINT: &str = "vw-hint";
    /// A field whose control is a switch: label left, switch right, one tappable row.
    pub const FIELD_TOGGLE: &str = "vw-field-toggle";

    pub const RELATION: &str = "vw-relation";
    pub const RELATION_ITEM: &str = "vw-relation-item";
    pub const CHAT: &str = "vw-chat";
    pub const CHAT_LOG: &str = "vw-chat-log";
    pub const MESSAGE: &str = "vw-message";
    pub const MESSAGE_META: &str = "vw-message-meta";
    pub const MESSAGE_BODY: &str = "vw-message-body";
    pub const MESSAGE_USER: &str = "vw-message-user";
    pub const MESSAGE_MODULE: &str = "vw-message-module";
    pub const MESSAGE_LIVE: &str = "vw-message-live";
    pub const STEPS: &str = "vw-steps";
    pub const STEP: &str = "vw-step";
    pub const STEP_NAME: &str = "vw-step-name";
    pub const STEP_TIME: &str = "vw-step-time";
    pub const STEP_RESUME: &str = "vw-step-resume";
    pub const STEP_MARK: &str = "vw-step-mark";
    pub const STEP_STATE: &str = "vw-step-state";
    pub const STEP_GROUP: &str = "vw-step-group";
    pub const STEP_GROUP_SUMMARY: &str = "vw-step-group-summary";
    pub const OLDER: &str = "vw-older";
    pub const QUESTION: &str = "vw-question";
    pub const QUESTION_TEXT: &str = "vw-question-text";
    pub const QUESTION_CHOICES: &str = "vw-question-choices";
    pub const QUESTION_CHOICE: &str = "vw-question-choice";
    pub const QUESTION_ANSWER: &str = "vw-question-answer";
    pub const QUESTION_FREE: &str = "vw-question-free";
    pub const COMPOSER: &str = "vw-composer";
    pub const COMPOSER_ACTIONS: &str = "vw-composer-actions";
    pub const COMPOSER_SLOT: &str = "vw-composer-slot";
    pub const SETTINGS: &str = "vw-settings";
    pub const SETTINGS_SECTION: &str = "vw-settings-section";
    pub const SETTINGS_TITLE: &str = "vw-settings-title";
    pub const SETTINGS_HINT: &str = "vw-settings-hint";
    pub const SETTINGS_ITEM: &str = "vw-settings-item";
    pub const TOOLSTEP: &str = "vw-toolstep";
    pub const QUEUE: &str = "vw-queue";
    pub const QUEUE_ITEM: &str = "vw-queue-item";
    pub const QUEUE_INDEX: &str = "vw-queue-index";
    pub const CODE: &str = "vw-code";
    pub const RESULT: &str = "vw-result";
    pub const IMAGE: &str = "vw-image";
    pub const IMAGE_UNAVAILABLE: &str = "vw-image-unavailable";
    pub const CAPABILITY: &str = "vw-capability";
    pub const CAPABILITY_ACTIONS: &str = "vw-capability-actions";
    pub const PROGRAM: &str = "vw-program";
    pub const PROGRAM_ARGS: &str = "vw-program-args";
    pub const COLUMNS: &str = "vw-columns";
    pub const COLUMN: &str = "vw-column";
    pub const CONFIRM: &str = "vw-confirm";

    pub const STATE: &str = "vw-state";
    pub const LOADING: &str = "vw-loading";
    pub const SKELETON: &str = "vw-skeleton";
    pub const EMPTY: &str = "vw-empty";
    pub const FAILED: &str = "vw-failed";
    pub const DENIED: &str = "vw-denied";
    pub const MISSING: &str = "vw-missing";
    pub const UNRENDERED: &str = "vw-unrendered";

    pub const ACCENT_NEUTRAL: &str = "vw-accent-neutral";
    pub const ACCENT_PRIMARY: &str = "vw-accent-primary";
    pub const ACCENT_SUCCESS: &str = "vw-accent-success";
    pub const ACCENT_WARNING: &str = "vw-accent-warning";
    pub const ACCENT_DANGER: &str = "vw-accent-danger";
    pub const ACCENT_INFO: &str = "vw-accent-info";
    pub const LAYOUT_STACK: &str = "vw-layout-stack";
    pub const LAYOUT_GRID: &str = "vw-layout-grid";
    pub const LAYOUT_COMPACT: &str = "vw-layout-compact";
    pub const TRANSITION_NONE: &str = "vw-transition-none";
    pub const TRANSITION_FADE: &str = "vw-transition-fade";
    pub const TRANSITION_SLIDE: &str = "vw-transition-slide";
}

/// Every class the renderer draws. Must mirror [`class`].
pub const ALL: &[&str] = &[
    class::SURFACE,
    class::SIDEBAR,
    class::HEADER,
    class::MAIN,
    class::PANEL,
    class::VIEW,
    class::VIEW_TITLE,
    class::VIEW_BODY,
    class::SR_ONLY,
    class::STACK,
    class::ROW,
    class::TABS,
    class::TABLIST,
    class::TAB,
    class::TABPANEL,
    class::DRAWER,
    class::DRAWER_BODY,
    class::MODAL,
    class::MODAL_HEAD,
    class::MODAL_BODY,
    class::MODAL_LAYER,
    class::MODAL_BACKDROP,
    class::TEXT,
    class::CAPTION,
    class::CHART,
    class::CHART_TITLE,
    class::CHART_SCROLL,
    class::BUTTON,
    class::ICON_BUTTON,
    class::BANNER,
    class::TONE_INFO,
    class::TONE_SUCCESS,
    class::TONE_WARNING,
    class::TONE_ERROR,
    class::INDICATOR,
    class::DOT,
    class::ACTIVITY,
    class::METRICS,
    class::LIST,
    class::LIST_ROW,
    class::LIST_ROW_CLICK,
    class::LIST_ACTIONS,
    class::LIST_LABEL,
    class::LIST_SUB,
    class::LIST_TOOLS,
    class::SEARCH,
    class::PAGER,
    class::PAGER_INFO,
    class::CARD,
    class::CARD_ROW,
    class::CARD_KEY,
    class::CARD_VALUE,
    class::FORM,
    class::FIELD,
    class::FIELD_TOGGLE,
    class::LABEL,
    class::INPUT,
    class::TEXTAREA,
    class::SELECT,
    class::TOGGLE,
    class::FILE,
    class::HINT,
    class::RELATION,
    class::RELATION_ITEM,
    class::CHAT,
    class::CHAT_LOG,
    class::MESSAGE,
    class::MESSAGE_META,
    class::MESSAGE_BODY,
    class::MESSAGE_USER,
    class::MESSAGE_MODULE,
    class::MESSAGE_LIVE,
    class::STEPS,
    class::STEP,
    class::STEP_NAME,
    class::STEP_TIME,
    class::STEP_RESUME,
    class::STEP_MARK,
    class::STEP_STATE,
    class::STEP_GROUP,
    class::STEP_GROUP_SUMMARY,
    class::OLDER,
    class::QUESTION,
    class::QUESTION_TEXT,
    class::QUESTION_CHOICES,
    class::QUESTION_CHOICE,
    class::QUESTION_ANSWER,
    class::QUESTION_FREE,
    class::COMPOSER,
    class::COMPOSER_ACTIONS,
    class::COMPOSER_SLOT,
    class::SETTINGS,
    class::SETTINGS_SECTION,
    class::SETTINGS_TITLE,
    class::SETTINGS_HINT,
    class::SETTINGS_ITEM,
    class::TOOLSTEP,
    class::QUEUE,
    class::QUEUE_ITEM,
    class::QUEUE_INDEX,
    class::CODE,
    class::RESULT,
    class::IMAGE,
    class::IMAGE_UNAVAILABLE,
    class::CAPABILITY,
    class::CAPABILITY_ACTIONS,
    class::PROGRAM,
    class::PROGRAM_ARGS,
    class::COLUMNS,
    class::COLUMN,
    class::CONFIRM,
    class::STATE,
    class::LOADING,
    class::SKELETON,
    class::EMPTY,
    class::FAILED,
    class::DENIED,
    class::MISSING,
    class::UNRENDERED,
    class::ACCENT_NEUTRAL,
    class::ACCENT_PRIMARY,
    class::ACCENT_SUCCESS,
    class::ACCENT_WARNING,
    class::ACCENT_DANGER,
    class::ACCENT_INFO,
    class::LAYOUT_STACK,
    class::LAYOUT_GRID,
    class::LAYOUT_COMPACT,
    class::TRANSITION_NONE,
    class::TRANSITION_FADE,
    class::TRANSITION_SLIDE,
];

/// The declared accent, as a class.
pub fn accent(accent: Accent) -> &'static str {
    match accent {
        Accent::Neutral => class::ACCENT_NEUTRAL,
        Accent::Primary => class::ACCENT_PRIMARY,
        Accent::Success => class::ACCENT_SUCCESS,
        Accent::Warning => class::ACCENT_WARNING,
        Accent::Danger => class::ACCENT_DANGER,
        Accent::Info => class::ACCENT_INFO,
    }
}

/// The declared layout of a root container, as a class.
pub fn layout(layout: Layout) -> &'static str {
    match layout {
        Layout::Stack => class::LAYOUT_STACK,
        Layout::Grid => class::LAYOUT_GRID,
        Layout::Compact => class::LAYOUT_COMPACT,
    }
}

/// The declared transition, as a class.
pub fn transition(transition: Transition) -> &'static str {
    match transition {
        Transition::None => class::TRANSITION_NONE,
        Transition::Fade => class::TRANSITION_FADE,
        Transition::Slide => class::TRANSITION_SLIDE,
    }
}

/// The sheet of the view renderer. Insert it once in the host page.
///
/// It defines everything the renderer needs to be usable: no dependency on the app's own
/// design tokens (every `var()` has a fallback), clickable zones of at least 44 px, a
/// visible focus ring, and a reduced-motion path.
pub const CSS: &str = r#"
.vw-surface { display: flex; flex-direction: column; gap: 12px; min-width: 0;
  color: var(--vw-ink, #16181d); background: var(--vw-bg, transparent); }
.vw-header { flex-direction: row; flex-wrap: wrap; align-items: center; gap: 16px;
  padding: 8px 12px; }
.vw-sidebar { padding: 12px; border-right: 1px solid var(--vw-line, rgba(0,0,0,.10)); }
.vw-panel { padding: 12px; border-left: 1px solid var(--vw-line, rgba(0,0,0,.10)); }
.vw-main { padding: 12px; }

.vw-accent-neutral { --vw-accent: #61646b; }
.vw-accent-primary { --vw-accent: #2563eb; }
.vw-accent-success { --vw-accent: #16a34a; }
.vw-accent-warning { --vw-accent: #b45309; }
.vw-accent-danger { --vw-accent: #b91c1c; }
.vw-accent-info { --vw-accent: #0e7490; }

.vw-view { display: flex; flex-direction: column; gap: 12px; }
.vw-view-title { margin: 0; font: 600 15px/1.3 inherit; letter-spacing: .01em; }
.vw-view-body { display: flex; flex-direction: column; gap: 12px; }
.vw-view-body[hidden] { display: none; }
.vw-sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px;
  overflow: hidden; clip-path: inset(50%); white-space: nowrap; border: 0; }
.vw-stack { display: flex; flex-direction: column; gap: 12px; }
.vw-row { display: flex; flex-direction: row; flex-wrap: wrap; gap: 12px; align-items: flex-start; }
.vw-layout-stack { display: flex; flex-direction: column; }
.vw-layout-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px; }
.vw-layout-compact { gap: 4px; }
.vw-transition-fade { animation: vw-fade 160ms ease-out both; }
.vw-transition-slide { animation: vw-slide 180ms ease-out both; }
.vw-transition-none { animation: none; }
@keyframes vw-fade { from { opacity: 0 } to { opacity: 1 } }
@keyframes vw-slide { from { opacity: 0; transform: translateY(6px) } to { opacity: 1; transform: none } }

.vw-tabs { display: flex; flex-direction: column; gap: 8px; }
.vw-tablist { display: flex; gap: 4px; overflow-x: auto; border-bottom: 1px solid var(--vw-line, rgba(0,0,0,.10)); }
.vw-tab { min-height: 44px; padding: 0 14px; border: 0; border-bottom: 2px solid transparent;
  background: none; font: inherit; color: inherit; cursor: pointer; }
.vw-tab[aria-selected="true"] { border-bottom-color: var(--vw-accent, #2563eb); font-weight: 600; }
.vw-tabpanel { padding: 8px 0; }

.vw-drawer { border: 1px solid var(--vw-line, rgba(0,0,0,.10)); border-radius: 10px; }
.vw-drawer > summary { display: flex; align-items: center; min-height: 44px; padding: 0 12px;
  font-weight: 600; cursor: pointer; }
.vw-drawer-body { padding: 8px 12px 12px; display: flex; flex-direction: column; gap: 8px; }

.vw-modal-layer { position: fixed; inset: 0; z-index: 1000; display: flex;
  align-items: center; justify-content: center; padding: 16px; }
.vw-modal-layer[hidden] { display: none; }
.vw-modal-backdrop { position: absolute; inset: 0;
  background: var(--vw-backdrop, rgba(0,0,0,.5)); }
.vw-modal { position: relative; z-index: 1; max-width: 100%; max-height: 100%; overflow: auto;
  border: 1px solid var(--vw-line, rgba(0,0,0,.12)); border-radius: 14px;
  background: var(--vw-card, #fff); color: var(--vw-ink, #16181d);
  box-shadow: 0 18px 48px rgba(0,0,0,.18); padding: 12px; }
.vw-modal-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.vw-modal-head h3 { margin: 0; font: 600 15px/1.3 inherit; }
.vw-modal-body { display: flex; flex-direction: column; gap: 12px; }

.vw-text { font: inherit; }
.vw-caption { display: block; margin: 0; padding: 0; border: 0; background: none;
  font-size: .8em; line-height: 1.3; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  color: var(--vw-muted, color-mix(in srgb, currentColor 60%, transparent)); }
.vw-chart { display: flex; flex-direction: column; gap: 4px; min-width: 0; max-width: 100%; }
.vw-chart-title { font-size: .8em; line-height: 1.3; margin: 0;
  color: var(--vw-muted, color-mix(in srgb, currentColor 60%, transparent)); }
.vw-chart-scroll { min-width: 0; max-width: 100%; overflow-x: auto; }
.vw-chart-scroll svg { display: block; }
.vw-btn { min-height: 44px; min-width: 44px; padding: 0 14px; border-radius: 10px;
  border: 1px solid var(--vw-line, rgba(0,0,0,.16)); background: var(--vw-card, #fff);
  color: inherit; font: inherit; cursor: pointer; }
.vw-btn[disabled] { opacity: .5; cursor: not-allowed; }
.vw-icon-btn { min-height: 44px; min-width: 44px; display: inline-flex; align-items: center;
  justify-content: center; border: 1px solid transparent; border-radius: 10px; background: none;
  color: inherit; cursor: pointer; }
.vw-confirm { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }

.vw-banner { display: flex; flex-wrap: wrap; gap: 8px; align-items: flex-start; margin: 0;
  padding: 10px 12px; border-radius: 10px; border: 1px solid var(--vw-line, rgba(0,0,0,.12));
  color: var(--vw-ink); }
/* Le texte d'un avis cède la place, son bouton jamais. `.vw-btn` porte `min-width: 44px`,
   qui REMPLACE le `min-width: auto` d'un enfant flex : sans la règle du bouton, un avis long
   le comprimait à 44 px et son libellé sortait du cadre (mesuré à 390 px : « Continuer »
   dépassait de 19 px, « Configurer » de 44 px). Le texte se replie sur une base de 14em :
   quand elle ne tient plus à côté du bouton (320 px), le bouton passe dessous, à droite. */
.vw-banner > span { flex: 1 1 14em; min-width: 0; overflow-wrap: anywhere; white-space: pre-line; }
.vw-banner > .vw-btn { flex: 0 0 auto; margin-left: auto; white-space: nowrap; }
/* Les tons se pilotent par variables : une couleur claire codée en dur donnait un pavé
   blanc au texte pâle sur un thème sombre (mesuré sur l'appareil : « 28 messages ·
   ~2718 jetons » illisible). Les valeurs de repli restent celles d'un thème clair. */
.vw-tone-info { border-color: var(--vw-tone-info-line, #b6d4fe);
  background: var(--vw-tone-info-bg, #eff6ff); color: var(--vw-tone-info-ink, #0b2b5b); }
.vw-tone-success { border-color: var(--vw-tone-success-line, #a7e0bd);
  background: var(--vw-tone-success-bg, #effaf3); color: var(--vw-tone-success-ink, #0d3b25); }
.vw-tone-warning { border-color: var(--vw-tone-warning-line, #f3d59b);
  background: var(--vw-tone-warning-bg, #fff9ec); color: var(--vw-tone-warning-ink, #4a3508); }
.vw-tone-error { border-color: var(--vw-tone-error-line, #f2b8b5);
  background: var(--vw-tone-error-bg, #fdf1f0); color: var(--vw-tone-error-ink, #5b1512); }

.vw-indicator { display: inline-flex; align-items: center; gap: 8px; min-height: 32px; font: inherit; }
.vw-dot { width: 8px; height: 8px; border-radius: 50%; background: currentColor; flex: 0 0 auto; }
.vw-activity { display: inline-flex; align-items: center; gap: 8px; }
.vw-activity .vw-dot { animation: vw-pulse 1.1s ease-in-out infinite; }
@keyframes vw-pulse { 0%,100% { opacity: .35 } 50% { opacity: 1 } }
.vw-metrics { display: flex; gap: 12px; flex-wrap: wrap; }

.vw-list { display: flex; flex-direction: column; gap: 8px; margin: 0; padding: 0; list-style: none; }
.vw-list-tools { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.vw-search { min-height: 44px; padding: 0 12px; border-radius: 10px; font: inherit;
  border: 1px solid var(--vw-line, rgba(0,0,0,.16)); background: var(--vw-card, #fff); color: inherit; }
.vw-list > li { margin: 0; }
.vw-list-row { display: flex; flex-direction: column; align-items: flex-start; justify-content: center;
  gap: 2px; width: 100%; min-height: 44px; padding: 8px 12px; text-align: left;
  border: 1px solid var(--vw-line, rgba(0,0,0,.10)); border-radius: 10px;
  background: var(--vw-card, #fff); color: inherit; font: inherit; cursor: pointer; }
.vw-list-row:hover { background: var(--vw-hover, rgba(0,0,0,.03)); }
/* Le bouton qui ouvre la ligne, quand `on_click` est déclaré : transparent, la rangée
   (`.vw-list-row`) porte déjà la bordure. Les boutons d'action de ligne en sont des frères,
   pas des enfants : un clic sur une action n'ouvre pas la ligne. */
.vw-list-row-click { display: flex; flex-direction: column; align-items: flex-start; gap: 2px;
  width: 100%; min-height: 44px; padding: 0; text-align: left; border: 0; background: none;
  color: inherit; font: inherit; cursor: pointer; }
.vw-list-actions { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 8px; align-items: center; }
.vw-list-actions .vw-btn { width: 100%; padding: 0 8px; }
.vw-list-label { font-weight: 600; }
.vw-list-sub { font-size: 13px; opacity: .72; }
.vw-pager { display: flex; gap: 8px; align-items: center; }
.vw-pager-info { font-size: 13px; opacity: .72; }

.vw-card { display: flex; flex-direction: column; gap: 4px; margin: 0;
  border: 1px solid var(--vw-line, rgba(0,0,0,.10)); border-radius: 12px; padding: 12px;
  background: var(--vw-card, #fff); }
.vw-card-row { display: flex; gap: 12px; align-items: baseline; justify-content: space-between; }
.vw-card-key { margin: 0; font-size: 13px; opacity: .72; }
.vw-card-value { margin: 0; font-weight: 500; text-align: right; }

.vw-form { display: flex; flex-direction: column; gap: 12px; }
.vw-field { display: flex; flex-direction: column; gap: 4px; }
.vw-label { font-size: 13px; font-weight: 600; }
.vw-hint { font-size: 12px; opacity: .7; }
.vw-input, .vw-select, .vw-file { min-height: 44px; padding: 0 12px; border-radius: 10px;
  border: 1px solid var(--vw-line, rgba(0,0,0,.16)); background: var(--vw-card, #fff);
  color: inherit; font: inherit; }
.vw-textarea { min-height: 88px; padding: 10px 12px; border-radius: 10px; font: inherit;
  border: 1px solid var(--vw-line, rgba(0,0,0,.16)); background: var(--vw-card, #fff); color: inherit; }
.vw-field-toggle { flex-direction: row; align-items: center; justify-content: space-between; gap: 12px;
  min-height: 44px; cursor: pointer; }
.vw-field-toggle .vw-label { font-size: 15px; font-weight: 500; flex: 1; min-width: 0; }
.vw-toggle { appearance: none; -webkit-appearance: none; flex: none; margin: 0; cursor: pointer;
  width: 51px; height: 31px; min-width: 51px; min-height: 31px; border-radius: 16px; position: relative;
  background: var(--vw-line, rgba(0,0,0,.2)); transition: background .2s ease; }
.vw-toggle::after { content: ""; position: absolute; top: 2px; left: 2px; width: 27px; height: 27px;
  border-radius: 50%; background: #fff; box-shadow: 0 2px 4px rgba(0,0,0,.3); transition: transform .2s ease; }
.vw-toggle:checked { background: var(--vw-accent, #34c759); }
.vw-toggle:checked::after { transform: translateX(20px); }
.vw-toggle:disabled { opacity: .5; cursor: default; }
.vw-input[readonly], .vw-textarea[readonly] { opacity: .7; }

.vw-relation { display: flex; flex-direction: column; gap: 8px; }
.vw-relation-item { display: flex; align-items: center; justify-content: space-between; gap: 8px;
  min-height: 44px; }
.vw-chat { display: flex; flex-direction: column; gap: 8px; }
.vw-chat-log { display: flex; flex-direction: column; gap: 8px; margin: 0; padding: 0; list-style: none; }
.vw-message { display: flex; flex-direction: column; gap: 2px; padding: 0; border-radius: 0;
  background: transparent; }
/* Une ligne du fil occupe TOUTE la largeur, quel que soit son texte : un texte court ne
   donne pas une ligne étroite. Seul le CONTENU est borné (la bulle de la personne : 92 %, à
   droite). */
.vw-chat-log > .vw-message { box-sizing: border-box; min-width: 0; width: 100%; }
.vw-chat-log > .vw-message-user { width: 92%; }
.vw-message-meta { font-size: 12px; opacity: .7; }
.vw-message-body { min-width: 0; overflow-wrap: anywhere; }
.vw-message-body > :first-child { margin-top: 0; }
.vw-message-body > :last-child { margin-bottom: 0; }
.vw-message-body p { margin: 0 0 8px; }
.vw-message-body pre { margin: 8px 0; padding: 12px; border-radius: 10px; overflow-x: auto;
  background: var(--vw-code-bg, rgba(0,0,0,.05)); font: 13px/1.5 ui-monospace, monospace; }
.vw-message-body code { font: 13px/1.5 ui-monospace, monospace; }
.vw-message-body table { border-collapse: collapse; margin: 8px 0; display: block; overflow-x: auto; }
.vw-message-body th, .vw-message-body td { border: 1px solid var(--vw-line, rgba(0,0,0,.12));
  padding: 4px 8px; text-align: left; }
.vw-message-body ul, .vw-message-body ol { margin: 8px 0; padding-left: 20px; }
/* Une réponse de l'agent se LIT : pas de cartouche autour d'elle, elle occupe la largeur et
   se suffit à elle-même. Le message de la personne reste une bulle, pour qu'on distingue qui
   parle d'un coup d'œil. Un fil où tout est encadré ne hiérarchise plus rien. */
.vw-message-user { padding: 8px 12px; border-radius: 12px; align-self: flex-end;
  width: 92%; max-width: 92%; background: var(--vw-user-bg, rgba(37,99,235,.10)); }
.vw-message-module { padding: 2px 0; background: transparent; align-self: stretch; }
.vw-message-live { opacity: .92; }
.vw-steps { display: flex; flex-direction: column; gap: 4px; margin: 6px 0 0; padding: 0;
  list-style: none; }
.vw-step { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 8px; min-width: 0;
  font-size: 13px; }
.vw-step-name { font-weight: 600; }
.vw-step > .vw-step-name { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
.vw-step-state { flex: 0 0 auto; font-size: 12px; opacity: .75; }
.vw-step-time { flex: 0 0 auto; font-variant-numeric: tabular-nums; white-space: nowrap;
  opacity: .75; }
/* Le résultat d'un outil se lit en trois lignes au plus : il renseigne, il ne se déverse pas. */
.vw-step-resume { flex-basis: 100%; min-width: 0; overflow-wrap: anywhere; opacity: .7;
  display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3; line-clamp: 3;
  overflow: hidden; }
/* La pastille d'une étape ou d'un groupe : son issue, lue dans `data-etat`. La couleur ne
   porte jamais le sens seule — le mot (« refusé », « en cours », « 2 en erreur ») est écrit.
   Trois variables que l'hôte peut poser : `--vw-ok`, `--vw-info`, `--vw-danger`. Pas
   `--vw-accent` pour « en cours » : c'est l'accent de la VUE, et une vue neutre le met en
   gris — un outil qui tourne avait alors la pastille d'un outil éteint (mesuré). */
.vw-step-mark { flex: 0 0 auto; width: 8px; height: 8px; border-radius: 50%;
  background: currentColor; opacity: .4; }
.vw-step[data-etat="fait"] > .vw-step-mark,
.vw-step-group[data-etat="fait"] > details > summary > .vw-step-mark {
  background: var(--vw-ok, #2e9e5b); opacity: 1; }
.vw-step[data-etat="en-cours"] > .vw-step-mark,
.vw-step-group[data-etat="en-cours"] > details > summary > .vw-step-mark {
  background: var(--vw-info, #2563eb); opacity: 1;
  animation: vw-pulse 1.1s ease-in-out infinite; }
.vw-step[data-etat="echec"] > .vw-step-mark, .vw-step[data-etat="refuse"] > .vw-step-mark,
.vw-step-group[data-etat="echec"] > details > summary > .vw-step-mark {
  background: var(--vw-danger, #d9534f); opacity: 1; }
.vw-step[data-etat="echec"] > .vw-step-state, .vw-step[data-etat="refuse"] > .vw-step-state {
  color: var(--vw-danger, #d9534f); font-weight: 600; opacity: 1; }
.vw-step[data-etat="en-cours"] > .vw-step-state { color: var(--vw-info, #2563eb);
  font-weight: 600; opacity: 1; }
/* Un groupe d'outils : un CADRE, replié sur une ligne qui dit combien, lesquels, comment ça
   s'est fini et en combien de temps ; ouvert sur demande, une ligne par outil. Le résultat
   brut d'un outil n'est PAS déversé dans le fil par défaut. Sans cadre, la ligne se
   confondait avec la prose de la réponse (mesuré : bordure 0, fond transparent). */
.vw-step-group { margin: 0; min-width: 0; overflow: hidden; border-radius: 10px;
  border: 1px solid var(--vw-line, rgba(0,0,0,.14)); background: var(--vw-card, rgba(0,0,0,.03)); }
.vw-step-group[data-etat="echec"] {
  border-color: color-mix(in srgb, var(--vw-danger, #d9534f) 45%, var(--vw-line, rgba(0,0,0,.14))); }
.vw-step-group > details > summary { cursor: pointer; user-select: none; list-style: none;
  display: flex; align-items: center; gap: 8px; min-width: 0; min-height: 44px;
  box-sizing: border-box; padding: 6px 12px; }
.vw-step-group > details > summary::-webkit-details-marker { display: none; }
.vw-step-group > details > summary::after { content: "⌄"; flex: 0 0 auto;
  font-size: 12px; opacity: .5; }
.vw-step-group > details[open] > summary::after { content: "⌃"; }
.vw-step-group > details[open] > summary {
  border-bottom: 1px solid var(--vw-line, rgba(0,0,0,.14)); }
.vw-step-group-summary { font-size: 13px; }
/* Les noms cèdent, la durée jamais : c'est elle qu'on vient lire. À 390 px trois noms ne
   tiennent pas sur une ligne à côté d'elle : ils se replient sur deux lignes au plus,
   plutôt que de se cacher derrière des points de suspension. */
.vw-step-group-summary > .vw-step-name { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere;
  display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2;
  overflow: hidden; }
.vw-step-group-summary > .vw-step-time { opacity: .85; }
.vw-step-group > details > .vw-steps { margin: 0; padding: 10px 12px; gap: 10px; }
.vw-older { min-height: 44px; width: 100%; border-radius: 10px;
  border: 1px dashed var(--vw-line, rgba(0,0,0,.16)); background: none; color: inherit;
  font: inherit; cursor: pointer; }
.vw-question { display: flex; flex-direction: column; gap: 8px; padding: 12px;
  border-radius: 12px; border: 1px solid var(--vw-line, rgba(0,0,0,.12));
  background: var(--vw-card, #fff); }
.vw-question-text { margin: 0; font-weight: 600; }
.vw-question-choices { display: flex; flex-wrap: wrap; gap: 8px; }
.vw-question-choice { min-height: 44px; padding: 0 14px; border-radius: 10px;
  border: 1px solid var(--vw-line, rgba(0,0,0,.16)); background: var(--vw-card, #fff);
  color: inherit; font: inherit; cursor: pointer; }
.vw-question-choice[disabled] { opacity: .5; cursor: not-allowed; }
.vw-question-answer { margin: 0; font-size: 13px; opacity: .8; }
.vw-question-free { display: flex; gap: 8px; align-items: center; }
.vw-composer { display: flex; gap: 8px; align-items: flex-end; }
.vw-composer-actions { display: flex; flex-wrap: wrap; gap: 8px; }
.vw-composer-slot { flex: 1; min-width: 0; min-height: 44px; }
.vw-settings { display: flex; flex-direction: column; gap: 24px; min-width: 0; }
.vw-settings-section { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.vw-settings-title { margin: 0; font-size: 14px; font-weight: 600; }
.vw-settings-hint { margin: 0; font-size: 12px; opacity: .7; }
.vw-settings-item { display: flex; flex-direction: column; align-items: flex-start; gap: 2px;
  min-height: 44px; width: 100%; min-width: 0; text-align: left; }
.vw-settings-item .vw-settings-hint { overflow-wrap: anywhere; }
.vw-toolstep { display: flex; align-items: center; gap: 8px; min-height: 32px; }

.vw-queue { display: flex; flex-direction: column; gap: 4px; margin: 0; padding: 0; list-style: none; }
.vw-queue-item { display: flex; gap: 8px; align-items: center; min-height: 44px; }
.vw-queue-index { min-width: 24px; font-variant-numeric: tabular-nums; opacity: .6; }
.vw-code { margin: 0; padding: 12px; border-radius: 10px; overflow-x: auto;
  background: var(--vw-code-bg, rgba(0,0,0,.05)); font: 13px/1.5 ui-monospace, monospace; }
.vw-result { margin: 0; padding: 12px; border-radius: 10px; overflow-x: auto; white-space: pre-wrap;
  border: 1px solid var(--vw-line, rgba(0,0,0,.10)); font: 13px/1.5 ui-monospace, monospace; }
.vw-image { display: block; max-width: 100%; border-radius: 10px; }
.vw-image-unavailable { display: flex; align-items: center; justify-content: center; min-height: 64px;
  padding: 12px; border-radius: 10px; color: var(--vw-muted, rgba(0,0,0,.55));
  border: 1px dashed var(--vw-line, rgba(0,0,0,.16)); }
.vw-capability { display: flex; flex-direction: column; gap: 8px; padding: 12px; border-radius: 12px;
  border: 1px solid var(--vw-line, rgba(0,0,0,.12)); background: var(--vw-card, #fff); }
.vw-capability-actions { display: flex; gap: 8px; flex-wrap: wrap; }
.vw-program { display: flex; flex-direction: column; gap: 12px; }
.vw-program-args { display: flex; flex-direction: column; gap: 8px; }
.vw-columns { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 8px; }
.vw-column { font-size: 13px; font-weight: 600; opacity: .75; }

.vw-state { display: flex; flex-direction: column; gap: 8px; padding: 16px; border-radius: 12px;
  border: 1px dashed var(--vw-line, rgba(0,0,0,.16)); }
.vw-loading { gap: 6px; }
.vw-skeleton { height: 12px; border-radius: 6px; background: var(--vw-hover, rgba(0,0,0,.06));
  animation: vw-pulse 1.2s ease-in-out infinite; }
.vw-skeleton:last-child { width: 60%; }
.vw-empty { color: var(--vw-muted, #61646b); }
.vw-failed { border-color: #f2b8b5; background: #fdf1f0; }
.vw-denied { border-color: #f3d59b; background: #fff9ec; }
.vw-missing { border-color: #c9cbd1; }
.vw-unrendered { border-style: dashed; border-color: #c9cbd1; font-size: 13px; opacity: .85; }

.vw-surface :is(a, button, input, select, textarea, summary, [tabindex]):focus-visible {
  outline: 2px solid var(--vw-accent, #2563eb); outline-offset: 2px; border-radius: 8px; }
.vw-surface :is(button, input, select, textarea, summary) { touch-action: manipulation; }

@media (prefers-reduced-motion: reduce) {
  .vw-transition-fade, .vw-transition-slide, .vw-skeleton, .vw-activity .vw-dot,
  .vw-step-mark { animation: none; }
}
@media (max-width: 700px) {
  .vw-row { flex-direction: column; align-items: stretch; }
  .vw-layout-grid { grid-template-columns: 1fr; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Une ligne du fil occupe toute la largeur (mesuré dans Chrome à 390 px : court et long
    /// donnent la même largeur) ; seule la bulle de la personne est bornée, à 92 %.
    #[test]
    fn les_lignes_du_fil_ont_toute_la_largeur() {
        assert!(CSS.contains(
            ".vw-chat-log > .vw-message { box-sizing: border-box; min-width: 0; width: 100%; }"
        ));
        assert!(CSS.contains(".vw-chat-log > .vw-message-user { width: 92%; }"));
    }

    /// Every `vw-…` token the sheet mentions.
    fn sheet_classes(css: &str) -> BTreeSet<String> {
        let bytes: Vec<char> = css.chars().collect();
        let mut found = BTreeSet::new();
        let mut i = 0;
        while i + 3 <= bytes.len() {
            // `var(--vw-line, …)` is a custom property, not a class: the token is skipped
            // when it follows a dash.
            let dashed = i > 0 && bytes[i - 1] == '-';
            if bytes[i] == 'v' && bytes[i + 1] == 'w' && bytes[i + 2] == '-' && !dashed {
                let mut j = i + 3;
                while j < bytes.len()
                    && (bytes[j].is_ascii_lowercase()
                        || bytes[j].is_ascii_digit()
                        || bytes[j] == '-')
                {
                    j += 1;
                }
                found.insert(bytes[i..j].iter().collect());
                i = j;
            } else {
                i += 1;
            }
        }
        found
    }

    #[test]
    fn every_declared_class_is_styled() {
        let missing: Vec<&str> = ALL
            .iter()
            .copied()
            .filter(|class| !CSS.contains(&format!(".{class}")))
            .collect();
        assert!(missing.is_empty(), "unstyled classes: {missing:?}");
    }

    /// The animation names the sheet defines: `vw-…` too, but not classes.
    fn keyframe_names(css: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut rest = css;
        while let Some(position) = rest.find("@keyframes ") {
            let after = &rest[position + "@keyframes ".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !name.is_empty() {
                names.insert(name);
            }
            rest = after;
        }
        names
    }

    #[test]
    fn the_sheet_styles_no_undeclared_class() {
        let declared: BTreeSet<String> = ALL.iter().map(|class| (*class).to_owned()).collect();
        let animations = keyframe_names(CSS);
        let unknown: Vec<String> = sheet_classes(CSS)
            .into_iter()
            .filter(|class| !declared.contains(class) && !animations.contains(class))
            .collect();
        assert!(unknown.is_empty(), "undeclared classes: {unknown:?}");
    }

    /// The declaration block of the first rule that starts with `selector` (alone or in a
    /// selector list), so `.vw-tab` is not confused with `.vw-tablist`.
    fn rule_of(css: &str, selector: &str) -> Option<String> {
        let mut rest = css;
        while let Some(position) = rest.find(selector) {
            let after = &rest[position + selector.len()..];
            let boundary =
                after.starts_with(',') || after.starts_with(" {") || after.starts_with('{');
            if boundary {
                let block = after.split('}').next().unwrap_or_default();
                return Some(block.to_owned());
            }
            rest = after;
        }
        None
    }

    #[test]
    fn clickable_zones_are_at_least_44_pixels() {
        for selector in [
            ".vw-btn",
            ".vw-icon-btn",
            ".vw-tab",
            ".vw-list-row",
            ".vw-list-row-click",
            ".vw-input",
            ".vw-select",
            ".vw-file",
            ".vw-search",
            ".vw-queue-item",
            // The switch itself is 31 px (the system's size); its ROW is the 44 px target.
            ".vw-field-toggle",
            ".vw-relation-item",
        ] {
            let block = rule_of(CSS, selector);
            let block = block.as_deref().unwrap_or_default();
            assert!(
                block.contains("min-height: 44px"),
                "{selector} lacks a 44px target: {block}"
            );
        }
        assert!(
            rule_of(CSS, ".vw-icon-btn")
                .unwrap_or_default()
                .contains("min-width: 44px")
        );
    }

    /// Le bouton d'un avis garde la largeur de son libellé : c'est le texte qui se replie.
    #[test]
    fn le_bouton_d_un_avis_ne_se_comprime_pas_sous_son_libelle() {
        let bouton = rule_of(CSS, ".vw-banner > .vw-btn").unwrap_or_default();
        for attendu in ["flex: 0 0 auto", "white-space: nowrap"] {
            assert!(
                bouton.contains(attendu),
                "bouton d'avis sans `{attendu}`: {bouton}"
            );
        }
        let texte = rule_of(CSS, ".vw-banner > span").unwrap_or_default();
        for attendu in ["min-width: 0", "overflow-wrap: anywhere"] {
            assert!(
                texte.contains(attendu),
                "texte d'avis sans `{attendu}`: {texte}"
            );
        }
        assert!(
            rule_of(CSS, ".vw-banner")
                .unwrap_or_default()
                .contains("flex-wrap: wrap"),
            "à 320 px le bouton doit pouvoir passer sous le texte"
        );
    }

    /// Un groupe d'outils est un CADRE (bordure, fond, coins arrondis), sa ligne repliée
    /// est une cible de 44 px, et chaque issue a sa pastille.
    #[test]
    fn un_groupe_d_outils_est_un_cadre_dont_la_ligne_se_touche() {
        let cadre = rule_of(CSS, ".vw-step-group").unwrap_or_default();
        for attendu in ["border: 1px solid", "border-radius: 10px", "background:"] {
            assert!(cadre.contains(attendu), "groupe sans `{attendu}`: {cadre}");
        }
        let ligne = rule_of(CSS, ".vw-step-group > details > summary").unwrap_or_default();
        assert!(
            ligne.contains("min-height: 44px"),
            "ligne repliée : {ligne}"
        );
        for etat in ["fait", "en-cours", "echec", "refuse"] {
            assert!(
                CSS.contains(&format!(".vw-step[data-etat=\"{etat}\"] > .vw-step-mark")),
                "aucune pastille pour l'issue `{etat}`"
            );
        }
    }

    #[test]
    fn a_caption_is_small_muted_and_has_no_frame_nor_background() {
        let block = rule_of(CSS, ".vw-caption").unwrap_or_default();
        for needed in [
            "font-size: .8em",
            "border: 0",
            "background: none",
            "margin: 0",
            "white-space: nowrap",
            "text-overflow: ellipsis",
            "--vw-muted",
        ] {
            assert!(block.contains(needed), "caption lacks `{needed}`: {block}");
        }
    }

    #[test]
    fn focus_is_visible_and_motion_can_be_reduced() {
        assert!(CSS.contains(":focus-visible"));
        assert!(CSS.contains("outline: 2px solid"));
        assert!(CSS.contains("prefers-reduced-motion"));
    }

    #[test]
    fn declaration_helpers_agree_with_the_contract_choices() {
        assert_eq!(accent(Accent::Danger), class::ACCENT_DANGER);
        assert_eq!(layout(Layout::Grid), class::LAYOUT_GRID);
        assert_eq!(transition(Transition::Fade), class::TRANSITION_FADE);
        let unique: BTreeSet<&str> = ALL.iter().copied().collect();
        assert_eq!(unique.len(), ALL.len(), "a class is declared twice");
    }
}
