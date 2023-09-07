use crate::{
    elements::{
        Code, Element, FormattedText, ListItem, ListItemType, PresentationMetadata, Text, TextChunk, TextFormat,
    },
    highlighting::{CodeHighlighter, CodeLine},
    media::MediaDrawer,
    presentation::Slide,
    resource::Resources,
    theme::{Alignment, AuthorPositioning, Colors, ElementStyle, ElementType, SlideTheme},
};
use crossterm::{
    cursor,
    style::{self, Stylize},
    terminal::{self, disable_raw_mode, enable_raw_mode, window_size, ClearType, WindowSize},
    QueueableCommand,
};
use std::{io, iter, mem};

pub type DrawResult = Result<(), DrawSlideError>;

pub struct Drawer<W: io::Write> {
    handle: W,
}

impl<W> Drawer<W>
where
    W: io::Write,
{
    pub fn new(mut handle: W) -> io::Result<Self> {
        enable_raw_mode()?;
        handle.queue(cursor::Hide)?;
        Ok(Self { handle })
    }

    pub fn draw_slide<'a>(
        &mut self,
        resources: &'a mut Resources,
        highlighter: &'a CodeHighlighter,
        theme: &'a SlideTheme,
        slide: &Slide,
    ) -> DrawResult {
        // Leave some room for eventual footer
        let mut dimensions = window_size()?;
        dimensions.rows -= 3;

        let slide_drawer = SlideDrawer { handle: &mut self.handle, resources, highlighter, theme, dimensions };
        slide_drawer.draw_slide(slide)
    }
}

impl<W> Drop for Drawer<W>
where
    W: io::Write,
{
    fn drop(&mut self) {
        let _ = self.handle.queue(cursor::Show);
        let _ = disable_raw_mode();
    }
}

struct SlideDrawer<'a, W> {
    handle: &'a mut W,
    resources: &'a mut Resources,
    highlighter: &'a CodeHighlighter,
    theme: &'a SlideTheme,
    dimensions: WindowSize,
}

impl<'a, W> SlideDrawer<'a, W>
where
    W: io::Write,
{
    fn draw_slide(mut self, slide: &Slide) -> DrawResult {
        self.apply_theme_colors()?;
        self.handle.queue(terminal::Clear(ClearType::All))?;
        self.handle.queue(cursor::MoveTo(0, 0))?;
        for element in &slide.elements {
            self.apply_theme_colors()?;
            self.draw_element(element)?;
        }
        self.handle.flush()?;
        Ok(())
    }

    fn apply_theme_colors(&mut self) -> io::Result<()> {
        apply_colors(self.handle, &self.theme.colors)
    }

    fn draw_element(&mut self, element: &Element) -> DrawResult {
        match element {
            Element::PresentationMetadata(metadata) => self.draw_presentation_metadata(metadata),
            Element::SlideTitle { text } => self.draw_slide_title(text),
            Element::Heading { text, level } => self.draw_heading(text, *level),
            Element::Paragraph(text) => self.draw_paragraph(text),
            Element::List(items) => self.draw_list(items),
            Element::Code(code) => self.draw_code(code),
        }
    }

    fn draw_presentation_metadata(&mut self, metadata: &PresentationMetadata) -> DrawResult {
        let center_row = self.dimensions.rows / 2;
        let title = Text {
            chunks: vec![TextChunk::Formatted(FormattedText::formatted(
                metadata.title.clone(),
                TextFormat::default().add_bold(),
            ))],
        };
        let sub_title = metadata
            .sub_title
            .as_ref()
            .map(|text| Text { chunks: vec![TextChunk::Formatted(FormattedText::plain(text.clone()))] });
        let author = metadata
            .author
            .as_ref()
            .map(|text| Text { chunks: vec![TextChunk::Formatted(FormattedText::plain(text.clone()))] });
        self.handle.queue(cursor::MoveToRow(center_row))?;
        self.draw_text(&title, ElementType::PresentationTitle)?;
        self.handle.queue(cursor::MoveToNextLine(1))?;
        if let Some(text) = sub_title {
            self.draw_text(&text, ElementType::PresentationSubTitle)?;
            self.handle.queue(cursor::MoveToNextLine(1))?;
        }
        if let Some(text) = author {
            match self.theme.author_positioning {
                AuthorPositioning::BelowTitle => {
                    self.handle.queue(cursor::MoveToNextLine(3))?;
                }
                AuthorPositioning::PageBottom => {
                    self.handle.queue(cursor::MoveToRow(self.dimensions.rows))?;
                }
            };
            self.draw_text(&text, ElementType::PresentationAuthor)?;
        }
        Ok(())
    }

    fn draw_slide_title(&mut self, text: &Text) -> DrawResult {
        self.handle.queue(cursor::MoveDown(1))?;
        self.handle.queue(style::SetAttribute(style::Attribute::Bold))?;
        self.draw_text(text, ElementType::SlideTitle)?;
        self.handle.queue(style::SetAttribute(style::Attribute::Reset))?;
        self.handle.queue(cursor::MoveToNextLine(2))?;

        let separator: String = "—".repeat(self.dimensions.columns as usize);
        self.apply_theme_colors()?;
        self.handle.queue(style::Print(separator))?;
        self.handle.queue(cursor::MoveToNextLine(2))?;
        Ok(())
    }

    fn draw_heading(&mut self, text: &Text, _level: u8) -> DrawResult {
        // TODO handle level
        self.handle.queue(style::SetAttribute(style::Attribute::Bold))?;
        // TODO
        self.draw_text(text, ElementType::Heading1)?;
        self.handle.queue(style::SetAttribute(style::Attribute::Reset))?;
        self.handle.queue(cursor::MoveToNextLine(2))?;
        Ok(())
    }

    fn draw_paragraph(&mut self, text: &Text) -> DrawResult {
        self.draw_text(text, ElementType::Paragraph)?;
        self.handle.queue(cursor::MoveToNextLine(2))?;
        Ok(())
    }

    fn draw_text(&mut self, text: &Text, parent_element: ElementType) -> DrawResult {
        let style = self.theme.style(&parent_element);
        let mut texts = Vec::new();
        for chunk in text.chunks.iter() {
            match chunk {
                TextChunk::Formatted(text) => {
                    texts.push(text);
                }
                TextChunk::Image { url, .. } => {
                    self.draw_formatted_texts(&mem::take(&mut texts), style)?;
                    self.draw_image(url)?;
                }
                TextChunk::LineBreak => {
                    self.draw_formatted_texts(&mem::take(&mut texts), style)?;
                    self.handle.queue(cursor::MoveToNextLine(1))?;
                }
            }
        }
        self.draw_formatted_texts(&mem::take(&mut texts), style)?;
        Ok(())
    }

    fn draw_formatted_texts(&mut self, text: &[&FormattedText], style: &ElementStyle) -> DrawResult {
        if text.is_empty() {
            return Ok(());
        }
        let text_drawer = TextDrawer::new(style, &mut self.handle, text, &self.dimensions, &self.theme.colors);
        text_drawer.draw()
    }

    fn draw_image(&mut self, path: &str) -> Result<(), DrawSlideError> {
        let image = self.resources.image(path).map_err(|e| DrawSlideError::Other(Box::new(e)))?;
        MediaDrawer.draw_image(&image, &self.dimensions).map_err(|e| DrawSlideError::Other(Box::new(e)))?;
        Ok(())
    }

    fn draw_list(&mut self, items: &[ListItem]) -> DrawResult {
        for item in items {
            self.draw_list_item(item)?;
        }
        self.handle.queue(cursor::MoveDown(2))?;
        Ok(())
    }

    fn draw_list_item(&mut self, item: &ListItem) -> DrawResult {
        let padding_length = (item.depth as usize + 1) * 2;
        let mut prefix: String = " ".repeat(padding_length);
        match item.item_type {
            ListItemType::Unordered => {
                let delimiter = match item.depth {
                    0 => '•',
                    1 => '◦',
                    _ => '▪',
                };
                prefix.push(delimiter);
            }
            ListItemType::OrderedParens(number) => {
                prefix.push_str(&number.to_string());
                prefix.push_str(") ");
            }
            ListItemType::OrderedPeriod(number) => {
                prefix.push_str(&number.to_string());
                prefix.push_str(". ");
            }
        };

        prefix.push(' ');
        let mut text = item.contents.clone();
        text.chunks.insert(0, TextChunk::Formatted(FormattedText::plain(prefix)));
        self.draw_text(&text, ElementType::List)?;
        self.handle.queue(cursor::MoveToNextLine(1))?;
        Ok(())
    }

    fn draw_code(&mut self, code: &Code) -> DrawResult {
        let style = self.theme.style(&ElementType::Code);
        let start_column = match style.alignment {
            Alignment::Left { margin } => margin,
            Alignment::Center { minimum_margin, minimum_size } => {
                let max_line_length =
                    code.contents.lines().map(|line| line.len()).max().unwrap_or(0).max(minimum_size as usize);
                let column = (self.dimensions.columns - max_line_length as u16) / 2;
                column.max(minimum_margin)
            }
        };
        self.handle.queue(cursor::MoveToColumn(start_column))?;

        let max_line_length = (self.dimensions.columns - start_column * 2) as usize;
        for code_line in self.highlighter.highlight(&code.contents, &code.language) {
            let CodeLine { original, mut formatted } = code_line;
            let line_length = original.len();
            let until_right_edge = max_line_length.saturating_sub(line_length);

            // Pad this code block with spaces so we get a nice little rectangle.
            formatted.pop();
            formatted.extend(iter::repeat(" ").take(until_right_edge));
            formatted.push('\n');
            self.handle.queue(style::Print(&formatted))?;
            self.handle.queue(cursor::MoveToColumn(start_column))?;
        }
        self.handle.queue(cursor::MoveDown(1))?;
        Ok(())
    }
}

struct TextDrawer<'a, W> {
    handle: &'a mut W,
    elements: &'a [&'a FormattedText],
    start_column: u16,
    line_length: u16,
    default_colors: &'a Colors,
}

impl<'a, W> TextDrawer<'a, W>
where
    W: io::Write,
{
    fn new(
        style: &'a ElementStyle,
        handle: &'a mut W,
        elements: &'a [&'a FormattedText],
        dimensions: &WindowSize,
        default_colors: &'a Colors,
    ) -> Self {
        let text_length: u16 = elements.iter().map(|chunk| chunk.text.len() as u16).sum();
        let mut line_length = dimensions.columns;
        let mut start_column;
        match style.alignment {
            Alignment::Left { margin } => {
                start_column = margin;
                line_length -= margin * 2;
            }
            Alignment::Center { minimum_margin, minimum_size } => {
                line_length = text_length.min(dimensions.columns - minimum_margin * 2).max(minimum_size);
                if line_length > dimensions.columns {
                    start_column = minimum_margin;
                } else {
                    start_column = (dimensions.columns - line_length) / 2;
                    start_column = start_column.max(minimum_margin);
                }
            }
        };
        Self { handle, elements, start_column, line_length, default_colors }
    }

    fn draw(self) -> DrawResult {
        let mut length_so_far = 0;
        self.handle.queue(cursor::MoveToColumn(self.start_column))?;
        for &element in self.elements {
            let (mut chunk, mut rest) = self.truncate(&element.text);
            loop {
                let mut styled = chunk.to_string().stylize();
                if element.format.has_bold() {
                    styled = styled.bold();
                }
                if element.format.has_italics() {
                    styled = styled.italic();
                }
                if element.format.has_code() {
                    styled = styled.italic();
                    if let Some(color) = self.default_colors.code {
                        styled = styled.with(color);
                    }
                }
                length_so_far += styled.content().len() as u16;
                if length_so_far > self.line_length {
                    self.handle.queue(cursor::MoveDown(1))?;
                    self.handle.queue(cursor::MoveToColumn(self.start_column))?;
                }
                self.handle.queue(style::PrintStyledContent(styled))?;
                apply_colors(self.handle, self.default_colors)?;
                if rest.is_empty() {
                    break;
                }
                (chunk, rest) = self.truncate(rest);
            }
        }
        Ok(())
    }

    fn truncate(&self, word: &'a str) -> (&'a str, &'a str) {
        let line_length = self.line_length as usize;
        if word.len() <= line_length {
            return (word, "");
        }
        let target_chunk = &word[0..line_length];
        let output_chunk = match target_chunk.rsplit_once(' ') {
            Some((before, _)) => before,
            None => target_chunk,
        };
        (output_chunk, word[output_chunk.len()..].trim())
    }
}

fn apply_colors<W: io::Write>(handle: &mut W, colors: &Colors) -> io::Result<()> {
    if let Some(color) = colors.background {
        handle.queue(style::SetBackgroundColor(color))?;
    }
    if let Some(color) = colors.foreground {
        handle.queue(style::SetForegroundColor(color))?;
    }
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum DrawSlideError {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error(transparent)]
    Other(Box<dyn std::error::Error>),
}
