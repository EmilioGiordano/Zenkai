use schemars::JsonSchema;
use serde::Deserialize;
use zenkai_datagen::GenerationSpec;
use zenkai_types::{BorderPreset, HAlign, NumberFormat, Rgb, StyleChange, WorkbookId};

use crate::tools::create::NewWorkbook;
use crate::tools::folder::InsidePath;

fn workbook_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<WorkbookId, D::Error> {
    u64::deserialize(deserializer).map(WorkbookId)
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkbooks {}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListSheets {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSelection {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadRange {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
    #[schemars(description = "Sheet name, as list_sheets returns it.")]
    pub sheet: String,
    #[schemars(description = "A1 range such as \"A1:D20\".")]
    pub range: String,
    #[serde(default)]
    #[schemars(description = "Page to read, from 0; long ranges are split into pages of rows.")]
    pub page: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Find {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
    #[schemars(description = "Text to look for in the shown cell values, ignoring case.")]
    pub text: String,
    #[serde(default)]
    #[schemars(description = "Only this sheet; every sheet when absent.")]
    pub sheet: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteCells {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
    pub sheet: String,
    #[schemars(description = "Top-left cell of the block, such as \"B2\".")]
    pub start: String,
    #[schemars(
        description = "Rows of cell entries, all of the same length. Each entry is typed as a user would type it: \"12\", \"text\" or \"=A1*2\"; an empty string clears the cell."
    )]
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetFormula {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
    pub sheet: String,
    #[schemars(description = "Cell such as \"C7\".")]
    pub cell: String,
    #[schemars(description = "Formula starting with \"=\", in Excel syntax.")]
    pub formula: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormatRange {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
    pub sheet: String,
    pub range: String,
    pub format: FormatChange,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateData {
    #[serde(deserialize_with = "workbook_id")]
    #[schemars(with = "u64", description = "Id from list_workbooks.")]
    pub workbook: WorkbookId,
    pub sheet: String,
    #[schemars(
        description = "Top-left cell of the table, such as \"A1\"; the header row goes here and the rows below it."
    )]
    pub start: String,
    pub spec: GenerationSpec,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkbook {
    #[schemars(
        description = "Path of the new .xlsx file relative to the working folder, such as \"budget.xlsx\" or \"reports/budget.xlsx\". Never absolute, never with \"..\"."
    )]
    pub path: String,
    #[serde(default)]
    #[schemars(description = "Sheet names in tab order; one sheet named Sheet1 when absent.")]
    pub sheets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenWorkbook {
    #[schemars(
        description = "Path of a spreadsheet relative to the working folder, such as \"sales.xlsx\"."
    )]
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormatChange {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Strikethrough(bool),
    Wrap(bool),
    FontSize(FontSize),
    NumberFormat(NumberFormatName),
    Align(Alignment),
    Borders(Borders),
    Fill(Option<HexColor>),
    FontColor(Option<HexColor>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(try_from = "u16")]
pub struct FontSize(#[schemars(range(min = 1, max = 409))] u16);

impl TryFrom<u16> for FontSize {
    type Error = String;

    fn try_from(points: u16) -> Result<FontSize, String> {
        if (1..=409).contains(&points) {
            Ok(FontSize(points))
        } else {
            Err(format!(
                "font size {points} is outside Excel's 1 to 409 points"
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(try_from = "String")]
pub struct HexColor(#[schemars(with = "String", regex(pattern = r"^#[0-9A-Fa-f]{6}$"))] Rgb);

impl TryFrom<String> for HexColor {
    type Error = String;

    fn try_from(text: String) -> Result<HexColor, String> {
        let valid = text.len() == 7 && text.starts_with('#');
        valid
            .then(|| Rgb::parse_hex(&text))
            .flatten()
            .map(HexColor)
            .ok_or_else(|| format!("colour \"{text}\" is not of the form #RRGGBB"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NumberFormatName {
    General,
    Number,
    Currency,
    Percent,
    Date,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    General,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Borders {
    All,
    Outside,
    Bottom,
    None,
}

impl FormatChange {
    pub fn style_change(self) -> StyleChange {
        match self {
            FormatChange::Bold(on) => StyleChange::Bold(on),
            FormatChange::Italic(on) => StyleChange::Italic(on),
            FormatChange::Underline(on) => StyleChange::Underline(on),
            FormatChange::Strikethrough(on) => StyleChange::Strike(on),
            FormatChange::Wrap(on) => StyleChange::Wrap(on),
            FormatChange::FontSize(FontSize(points)) => StyleChange::FontSize(points),
            FormatChange::NumberFormat(name) => StyleChange::NumberFormat(match name {
                NumberFormatName::General => NumberFormat::General,
                NumberFormatName::Number => NumberFormat::Number,
                NumberFormatName::Currency => NumberFormat::Currency,
                NumberFormatName::Percent => NumberFormat::Percent,
                NumberFormatName::Date => NumberFormat::Date,
                NumberFormatName::Time => NumberFormat::Time,
            }),
            FormatChange::Align(alignment) => StyleChange::Align(match alignment {
                Alignment::General => HAlign::General,
                Alignment::Left => HAlign::Left,
                Alignment::Center => HAlign::Center,
                Alignment::Right => HAlign::Right,
            }),
            FormatChange::Borders(borders) => StyleChange::Borders(match borders {
                Borders::All => BorderPreset::All,
                Borders::Outside => BorderPreset::Outside,
                Borders::Bottom => BorderPreset::Bottom,
                Borders::None => BorderPreset::None,
            }),
            FormatChange::Fill(color) => StyleChange::Fill(color.map(|c| c.0)),
            FormatChange::FontColor(color) => StyleChange::FontColor(color.map(|c| c.0)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReadRequest {
    ListSheets,
    ReadRange(ReadRange),
    Find(Find),
}

#[derive(Clone, Debug, PartialEq)]
pub enum WriteRequest {
    WriteCells(WriteCells),
    SetFormula(SetFormula),
    FormatRange(FormatRange),
    GenerateData(GenerateData),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolRequest {
    ListWorkbooks,
    GetSelection(WorkbookId),
    Read(WorkbookId, ReadRequest),
    Write(WorkbookId, WriteRequest),
    CreateWorkbook(NewWorkbook),
    OpenWorkbook(InsidePath),
}

impl From<ListSheets> for ToolRequest {
    fn from(request: ListSheets) -> ToolRequest {
        ToolRequest::Read(request.workbook, ReadRequest::ListSheets)
    }
}

impl From<GetSelection> for ToolRequest {
    fn from(request: GetSelection) -> ToolRequest {
        ToolRequest::GetSelection(request.workbook)
    }
}

impl From<ReadRange> for ToolRequest {
    fn from(request: ReadRange) -> ToolRequest {
        ToolRequest::Read(request.workbook, ReadRequest::ReadRange(request))
    }
}

impl From<Find> for ToolRequest {
    fn from(request: Find) -> ToolRequest {
        ToolRequest::Read(request.workbook, ReadRequest::Find(request))
    }
}

impl From<WriteCells> for ToolRequest {
    fn from(request: WriteCells) -> ToolRequest {
        ToolRequest::Write(request.workbook, WriteRequest::WriteCells(request))
    }
}

impl From<SetFormula> for ToolRequest {
    fn from(request: SetFormula) -> ToolRequest {
        ToolRequest::Write(request.workbook, WriteRequest::SetFormula(request))
    }
}

impl From<FormatRange> for ToolRequest {
    fn from(request: FormatRange) -> ToolRequest {
        ToolRequest::Write(request.workbook, WriteRequest::FormatRange(request))
    }
}

impl From<GenerateData> for ToolRequest {
    fn from(request: GenerateData) -> ToolRequest {
        ToolRequest::Write(request.workbook, WriteRequest::GenerateData(request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse_from_their_json_form() {
        let request: FormatRange = serde_json::from_str(
            r##"{ "workbook": 3, "sheet": "S", "range": "A1:B2", "format": { "fill": "#FF8800" } }"##,
        )
        .unwrap();
        assert_eq!(
            request.format.style_change(),
            StyleChange::Fill(Some(Rgb(0xFF8800)))
        );
        let none: FormatChange = serde_json::from_str(r#"{ "fill": null }"#).unwrap();
        assert_eq!(none.style_change(), StyleChange::Fill(None));
        for bad in [
            r#"{ "fill": "red" }"#,
            r#"{ "font_size": 0 }"#,
            r#"{ "number_format": "roman" }"#,
        ] {
            assert!(serde_json::from_str::<FormatChange>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unknown_arguments_are_refused() {
        let extra = r#"{ "workbook": 1, "sheet": "S", "range": "A1", "path": "C:\\x" }"#;
        assert!(serde_json::from_str::<ReadRange>(extra).is_err());
    }
}
