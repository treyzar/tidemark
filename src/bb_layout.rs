#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
pub struct Layout {
    pub question: Rect,
    pub options: Rect,
    pub reply: Rect,
    pub footer: i32,
}
pub fn layout(width: i32, height: i32, options: usize) -> Layout {
    let footer = height - 80;
    let h = if options > 0 {
        (88 + (height - 620) / 3).clamp(88, 132)
    } else {
        (136 + (height - 620) / 3).clamp(136, 184)
    };
    let question = Rect {
        x: 28,
        y: 160,
        w: width - 56,
        h,
    };
    let top = question.y + question.h + 12;
    let max_options = (footer - 20 - 64 - 36 - top).max(0);
    let choices = Rect {
        x: 28,
        y: top,
        w: width - 56,
        h: (options.min(3) as i32 * 64).min(max_options),
    };
    let reply_y = if options > 0 {
        choices.y + choices.h + 36
    } else {
        question.y + question.h + 40
    };
    Layout {
        question,
        options: choices,
        reply: Rect {
            x: 28,
            y: reply_y,
            w: width - 56,
            h: footer - 20 - reply_y,
        },
        footer,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_fits_small_windows_without_overlapping_footer() {
        for height in [620, 680, 740] {
            for count in [0, 1, 2, 3, 10] {
                let l = layout(740, height, count);
                assert!(l.question.h >= 80);
                assert!(l.reply.h >= 64);
                assert!(l.reply.y + l.reply.h <= l.footer - 16);
                if count > 0 {
                    assert!(l.options.y >= l.question.y + l.question.h + 8);
                    assert!(l.reply.y >= l.options.y + l.options.h + 32);
                }
            }
        }
    }
    #[test]
    fn composer_uses_space_released_by_absent_choices() {
        assert!(layout(740, 740, 0).reply.h > layout(740, 740, 3).reply.h);
    }
}
