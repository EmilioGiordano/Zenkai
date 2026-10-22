use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use pdfium_render::prelude::Pdfium;

use crate::codec::{CodecError, decode_request, encode_reply};
use crate::frame::{FrameError, read_frame, write_frame};
use crate::wire::{Job, OpenError, Reply};

mod document;
mod library;
mod outline;
mod page_text;
#[cfg(test)]
mod tests;

use document::{Documents, failure_reply, process};

// A test sets this in the environment of the host it starts, so that the abort request
// has an effect there and nowhere else.
pub const ABORT_ENVIRONMENT: &str = "ZENKAI_PDF_HOST_ALLOW_ABORT";

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Codec(#[from] CodecError),
}

// Runs until the app closes the pipe. Anything that is not a well formed request ends the
// host, since the app is the only thing that talks to it.
pub fn run_stdio() -> Result<(), HostError> {
    let library = library::bind_next_to_exe();
    serve(
        &library,
        std::io::BufReader::new(std::io::stdin().lock()),
        std::io::stdout().lock(),
    )
}

pub fn serve(
    library: &Result<Pdfium, OpenError>,
    mut input: impl Read,
    mut output: impl Write,
) -> Result<(), HostError> {
    let mut documents = Documents::new();
    while let Some(frame) = read_frame(&mut input)? {
        let (id, doc, job) = decode_request(&frame)?;
        if job == Job::Abort && std::env::var_os(ABORT_ENVIRONMENT).is_some() {
            std::process::abort();
        }
        let reply = match library {
            Ok(pdfium) => answer(pdfium, &mut documents, doc, &job),
            Err(error) => {
                matches!(job, Job::Open { .. }).then(|| Reply::Opened(Err(error.clone())))
            }
        };
        write_frame(&mut output, &encode_reply(id, reply.as_ref())?)?;
    }
    Ok(())
}

fn answer<'a>(
    pdfium: &'a Pdfium,
    documents: &mut Documents<'a>,
    doc: crate::wire::DocId,
    job: &Job,
) -> Option<Reply> {
    catch_unwind(AssertUnwindSafe(|| process(pdfium, documents, doc, job))).unwrap_or_else(|_| {
        tracing::error!("pdfium panicked while serving a request");
        failure_reply(job)
    })
}
