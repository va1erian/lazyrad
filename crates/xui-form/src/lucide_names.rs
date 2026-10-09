// Generated from `xui-app/crates/named-icons/src/names.rs` (the OS-wide
// stable Lucide names). Keep the two tables in step.

use xui_core::Lucide;

/// The Lucide outline a `Button`'s `icon` names, or `None` for an unknown or
/// empty name (the button then has no icon).
pub fn from_name(name: &str) -> Option<Lucide> {
    Some(match name {
        "app-window" => Lucide::AppWindow,
        "arrow-down-a-z" => Lucide::ArrowDownAZ,
        "between-horizontal-end" => Lucide::BetweenHorizontalEnd,
        "between-horizontal-start" => Lucide::BetweenHorizontalStart,
        "between-vertical-end" => Lucide::BetweenVerticalEnd,
        "between-vertical-start" => Lucide::BetweenVerticalStart,
        "bold" => Lucide::Bold,
        "book-open" => Lucide::BookOpen,
        "box" => Lucide::Box,
        "bug" => Lucide::Bug,
        "check" => Lucide::Check,
        "chevron-down" => Lucide::ChevronDown,
        "chevron-left" => Lucide::ChevronLeft,
        "chevron-right" => Lucide::ChevronRight,
        "chevron-up" => Lucide::ChevronUp,
        "chevrons-up-down" => Lucide::ChevronsUpDown,
        "circle-dot" => Lucide::CircleDot,
        "circle-help" => Lucide::CircleHelp,
        "circle-x" => Lucide::CircleX,
        "clipboard-paste" => Lucide::ClipboardPaste,
        "code" => Lucide::Code,
        "copy" => Lucide::Copy,
        "disc" => Lucide::Disc,
        "download" => Lucide::Download,
        "ellipsis" => Lucide::Ellipsis,
        "external-link" => Lucide::ExternalLink,
        "eye-off" => Lucide::EyeOff,
        "eye" => Lucide::Eye,
        "file-code" => Lucide::FileCode,
        "file-plus" => Lucide::FilePlus,
        "file-text" => Lucide::FileText,
        "file" => Lucide::File,
        "folder-open" => Lucide::FolderOpen,
        "folder" => Lucide::Folder,
        "group" => Lucide::Group,
        "heading-1" => Lucide::Heading1,
        "heading-2" => Lucide::Heading2,
        "heading-3" => Lucide::Heading3,
        "history" => Lucide::History,
        "home" => Lucide::Home,
        "image" => Lucide::Image,
        "inbox" => Lucide::Inbox,
        "indent-decrease" => Lucide::IndentDecrease,
        "indent-increase" => Lucide::IndentIncrease,
        "info" => Lucide::Info,
        "italic" => Lucide::Italic,
        "layout-grid" => Lucide::LayoutGrid,
        "link" => Lucide::Link,
        "list-ordered" => Lucide::ListOrdered,
        "list-tree" => Lucide::ListTree,
        "list" => Lucide::List,
        "lock" => Lucide::Lock,
        "log-out" => Lucide::LogOut,
        "mail" => Lucide::Mail,
        "menu" => Lucide::Menu,
        "minus" => Lucide::Minus,
        "monitor" => Lucide::Monitor,
        "mouse-pointer-2" => Lucide::MousePointer2,
        "package" => Lucide::Package,
        "pause" => Lucide::Pause,
        "pencil" => Lucide::Pencil,
        "play" => Lucide::Play,
        "plus" => Lucide::Plus,
        "printer" => Lucide::Printer,
        "rectangle-horizontal" => Lucide::RectangleHorizontal,
        "redo-2" => Lucide::Redo2,
        "refresh-cw" => Lucide::RefreshCw,
        "repeat" => Lucide::Repeat,
        "reply" => Lucide::Reply,
        "ruler" => Lucide::Ruler,
        "save-all" => Lucide::SaveAll,
        "save" => Lucide::Save,
        "scissors" => Lucide::Scissors,
        "search" => Lucide::Search,
        "send" => Lucide::Send,
        "separator-horizontal" => Lucide::SeparatorHorizontal,
        "settings" => Lucide::Settings,
        "shuffle" => Lucide::Shuffle,
        "skip-back" => Lucide::SkipBack,
        "skip-forward" => Lucide::SkipForward,
        "square-check" => Lucide::SquareCheck,
        "square-mouse-pointer" => Lucide::SquareMousePointer,
        "square" => Lucide::Square,
        "star" => Lucide::Star,
        "strikethrough" => Lucide::Strikethrough,
        "table" => Lucide::Table,
        "tag" => Lucide::Tag,
        "terminal" => Lucide::Terminal,
        "text-align-center" => Lucide::TextAlignCenter,
        "text-align-end" => Lucide::TextAlignEnd,
        "text-align-justify" => Lucide::TextAlignJustify,
        "text-align-start" => Lucide::TextAlignStart,
        "text-cursor-input" => Lucide::TextCursorInput,
        "text-quote" => Lucide::TextQuote,
        "trash-2" => Lucide::Trash2,
        "triangle-alert" => Lucide::TriangleAlert,
        "type" => Lucide::Type,
        "underline" => Lucide::Underline,
        "undo-2" => Lucide::Undo2,
        "unlock" => Lucide::Unlock,
        "upload" => Lucide::Upload,
        "users" => Lucide::Users,
        "volume-2" => Lucide::Volume2,
        "wrap-text" => Lucide::WrapText,
        "x" => Lucide::X,
        "zap" => Lucide::Zap,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_to_outlines() {
        assert_eq!(from_name("chevron-left"), Some(Lucide::ChevronLeft));
        assert_eq!(from_name("volume-2"), Some(Lucide::Volume2));
    }

    #[test]
    fn unknown_and_empty_names_have_no_outline() {
        assert_eq!(from_name(""), None);
        assert_eq!(from_name("no-such-icon"), None);
    }
}
