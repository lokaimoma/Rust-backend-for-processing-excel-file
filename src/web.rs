use crate::{
    data::{
        model::{RowsPayload, UploadFileEntry},
        sqlite_ds::SqliteDataSource,
        DataSource,
    },
    error::Error,
    Result,
};
use axum::{
    body::Bytes,
    extract::{Multipart, State},
    routing::post,
    Json, Router,
};
use calamine::{open_workbook_auto, DataType, Reader};
use serde_json::json;
use serde_json::Value;
use std::path::PathBuf;
use tokio::fs;

pub fn get_routes(datasource: SqliteDataSource) -> Router {
    Router::new()
        .route("/upload", post(upload_file))
        .route("/getHeader", post(get_header_row))
        .route("/runJob", post(run_job))
        .with_state(datasource)
}

struct JobDetails {
    file_id: String,
    contraction_file: Option<Bytes>,
    search_terms: Vec<String>,
    check_date: bool,
}

impl JobDetails {
    const FILE_ID_FIELD_N: &'static str = "fileId";
    const CONTRACTION_F_FIELD_N: &'static str = "contractionFile";
    const SEARCH_TERMS_FIELD_N: &'static str = "searchTerms";
    const CHECK_DATE_FIELD_N: &'static str = "checkDate";
    const SEARCH_TERM_COUNTER_LIMIT: usize = 5;

    fn file_id(&self) -> &str {
        &self.file_id
    }

    fn contraction_file(&self) -> &Option<Bytes> {
        &self.contraction_file
    }

    fn search_terms(&self) -> &Vec<String> {
        &self.search_terms
    }

    fn check_date(&self) -> bool {
        self.check_date
    }

    async fn try_from(mut value: Multipart) -> Result<Self> {
        let mut file_id: Option<String> = None;
        let mut contraction_file: Option<Bytes> = None;
        let mut search_terms: Vec<String> = Vec::with_capacity(5);
        let mut check_date: bool = false;

        let mut search_t_counter = 0;

        while let Some(field) = value.next_field().await? {
            let name = field.name();
            if name.is_none() {
                continue;
            }
            let name = name.unwrap();
            match name {
                JobDetails::FILE_ID_FIELD_N => file_id = Some(name.to_owned()),
                JobDetails::CONTRACTION_F_FIELD_N => {
                    let bytes = field.bytes().await?;
                    contraction_file = Some(bytes);
                }
                JobDetails::SEARCH_TERMS_FIELD_N => {
                    if search_t_counter < JobDetails::SEARCH_TERM_COUNTER_LIMIT {
                        let text = field.text().await?;
                        search_terms.insert(search_t_counter, text.to_owned());
                        search_t_counter += 1;
                    }
                }
                JobDetails::CHECK_DATE_FIELD_N => check_date = true,
                _ => {}
            }
        }
        if file_id.is_none() {
            return Err(Error::MultipartFormError(
                "fileId not present in formdata".to_string(),
            ));
        }
        Ok(Self {
            file_id: file_id.unwrap(),
            contraction_file,
            check_date,
            search_terms,
        })
    }
}

async fn run_job(mut multipart: Multipart) -> Result<Json<Value>> {
    let job_detail = JobDetails::try_from(multipart).await?;
    todo!()
}

async fn get_header_row(
    State(datasource): State<SqliteDataSource>,
    Json(partial_f_entry): Json<UploadFileEntry>,
) -> Result<Json<Value>> {
    let result = match datasource.get_file_entry(partial_f_entry.id).await {
        Ok(r) => r,
        Err(e) => return Err(Error::DatabaseOperationFailed(e.to_string())),
    };

    let mut workbook = match open_workbook_auto(result.file_path) {
        Err(e) => return Err(Error::IOError(e.to_string())),
        Ok(wb) => wb,
    };

    let first_sheet = workbook.worksheet_range_at(0);

    if first_sheet.is_none() {
        return Err(Error::Generic("No sheet found in excel file".into()));
    }

    let rows_data = match first_sheet.unwrap() {
        Err(e) => return Err(Error::Generic(e.to_string())),
        Ok(v) => v,
    };

    let rows: Vec<String> = rows_data
        .rows()
        .take(1)
        .flat_map(|r| {
            let mut data = Vec::new();
            for c in r {
                let s = match c {
                    DataType::String(s) => s.to_owned(),
                    DataType::Int(i) => i.to_string(),
                    DataType::Float(f) => f.to_string(),
                    DataType::Bool(b) => b.to_string(),
                    DataType::DateTime(d) => d.to_string(),
                    DataType::Duration(d) => d.to_string(),
                    DataType::DateTimeIso(dt) => dt.to_string(),
                    DataType::DurationIso(dt) => dt.to_string(),
                    DataType::Error(e) => e.to_string(),
                    DataType::Empty => "".to_owned(),
                };
                data.push(s);
            }
            data
        })
        .collect();

    let rows = RowsPayload { rows };
    let rows = json!(rows);

    Ok(Json(rows))
}

async fn upload_file(
    State(datasource): State<SqliteDataSource>,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    let result = multipart.next_field().await;

    if result.is_err() {
        return Err(Error::MultipartFormError(result.err().unwrap().body_text()));
    }

    let field = result.unwrap();

    if field.is_none() {
        return Err(Error::NoFileUploaded);
    }

    let field = field.unwrap();

    let fname = field.file_name();
    if fname.is_none() {
        return Err(Error::NoFileUploaded);
    }
    let fname = fname.unwrap().to_string();
    let bytes = field.bytes().await.unwrap();

    let mut file_path = PathBuf::from(".\\data");
    if !file_path.exists() {
        let _ = fs::create_dir_all(&file_path).await;
    }
    file_path.push(&fname);

    if let Err(e) = fs::write(&file_path, bytes).await {
        println!("Error writing file");
        eprintln!("{} : {:?}", e, file_path);
        return Err(Error::WritingToDisk(fname));
    };

    let id = datasource.add_file_entry(&file_path).await;

    if id.is_err() {
        return Err(id.err().unwrap());
    }

    let f_entry = UploadFileEntry {
        id: id.unwrap().into(),
        file_path: file_path.to_string_lossy().to_string(),
    };

    if let Err(e) = open_workbook_auto(&file_path) {
        let _ = fs::remove_file(file_path).await;
        return Err(Error::InValidExcelFile(e.to_string()));
    };

    Ok(Json(json!(f_entry)))
}
