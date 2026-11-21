// How a parsed Markdown text is drawn: a flat list of lines of plain GPUI text, from the
// tree that `markdown_depth` has measured. Neither the What's new notes nor the Markdown
// preview use gpui's Markdown viewer.
mod blocks;
mod draw;

pub use blocks::{Block, Mode, blocks_of};
pub use draw::{Look, OnLink, draw_block};
