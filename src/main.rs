use std::{borrow::Borrow, io::Cursor};

use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use axum_typed_multipart::{FieldData, TryFromMultipart, TypedMultipart};
use imageproc::image::{
    codecs::jpeg::JpegEncoder,
    error::EncodingError,
    imageops::{overlay, FilterType},
    DynamicImage, ImageError, ImageFormat, ImageReader,
};
use resvg::{
    tiny_skia::{self, IntSize},
    usvg::{self},
};
use serde::Serialize;
#[derive(TryFromMultipart)]
struct TransformRequest {
    image: FieldData<Bytes>,
    layers: Vec<FieldData<Bytes>>,
}
#[tokio::main]
async fn main() {
    // build our application with a single route
    let app = Router::new().route("/", post(create_document));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
enum AppError {
    MissingMimeType,
    InvalidMimeType(String),
    DecodingFailure(ImageError),
    EncodingFailure,
    SvgParserFailure,
    InvalidSize,
    RenderFailure(EncodingError),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // How we want errors responses to be serialized
        #[derive(Serialize)]
        struct ErrorResponse {
            #[serde(rename = "type")]
            //error_type: String,
            //status: i32,
            title: String,
            details: String,
            //instance: String,
        }

        let (status, message) = match self {
            AppError::DecodingFailure(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    title: "Decoding-Error".into(),
                    details: "Failed to decode one of the overlays".into(),
                },
            ),
            AppError::MissingMimeType => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    title: "MimeType-Error".into(),
                    details: "Missing mime type for one of the overlays".into(),
                },
            ),
            AppError::InvalidMimeType(mime_type) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    title: "MimeType-Error".into(),
                    details: format!("Invalid mime type ({})for one of the overlays", mime_type),
                },
            ),
            AppError::EncodingFailure => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    title: "Encoding-Error".into(),
                    details: "Failed to encode the image".into(),
                },
            ),
            AppError::SvgParserFailure => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    title: "Svg-Error".into(),
                    details: "Failed to parse one of the svg-overlays".into(),
                },
            ),
            AppError::InvalidSize => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    title: "Transform-Error".into(),
                    details: "The image or overlay has an invalid size".into(),
                },
            ),
            AppError::RenderFailure(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    title: "Svg-Error".into(),
                    details: "Failed to parse one of the svg-overlays".into(),
                },
            ),
        };

        (status, axum::Json(message)).into_response()
    }
}

fn prepare_layers(
    image_witdh: u32,
    image_height: u32,
) -> impl FnMut(&FieldData<Bytes>) -> Result<DynamicImage, AppError> {
    move |layer| {
        if layer.metadata.content_type.as_ref().unwrap() == "image/svg+xml" {
            let mut opt = usvg::Options::default();
            opt.fontdb_mut().load_system_fonts();

            let svg_data = layer.contents.clone();
            let tree =
                usvg::Tree::from_data(&svg_data, &opt).map_err(|_| AppError::SvgParserFailure)?;

            //let render_ts = tiny_skia::Transform::from_scale(zoom, zoom);
            let original_size = tree.size().to_int_size();
            let pixmap_size = tree.size().to_int_size().scale_to(
                IntSize::from_wh(image_witdh, image_height).ok_or(AppError::InvalidSize)?,
            );
            let mut pixmap = tiny_skia::Pixmap::new(pixmap_size.width(), pixmap_size.height())
                .ok_or(AppError::InvalidSize)?;
            let transfrom = tiny_skia::Transform::from_scale(
                image_witdh as f32 / original_size.width() as f32,
                image_height as f32 / original_size.height() as f32,
            );

            resvg::render(&tree, transfrom, &mut pixmap.as_mut());

            let rgba = pixmap.encode_png().map_err(|_| AppError::EncodingFailure)?;
            let mut overlay_reader =
                ImageReader::new(Cursor::new(Bytes::from_iter(rgba.into_iter())));
            overlay_reader.set_format(ImageFormat::Png);
            let mut overlay_image = overlay_reader
                .decode()
                .map_err(|err| AppError::DecodingFailure(err))?;
            overlay_image = overlay_image.resize(image_witdh, image_height, FilterType::Nearest);
            return Ok(overlay_image);
        } else {
            let mut overlay_reader = ImageReader::new(Cursor::new(layer.contents.clone()));
            let mimetype = layer.metadata.content_type.as_ref();
            let unwraped_mimetype = mimetype.ok_or(AppError::MissingMimeType)?;
            overlay_reader.set_format(
                ImageFormat::from_mime_type(unwraped_mimetype)
                    .ok_or(AppError::InvalidMimeType(unwraped_mimetype.into()))?,
            );
            let mut overlay_image = overlay_reader
                .decode()
                .map_err(|err| AppError::DecodingFailure(err))?;
            overlay_image = overlay_image.resize(image_witdh, image_height, FilterType::Nearest);
            return Ok(overlay_image);
        }
    }
}

async fn create_document(
    payload: TypedMultipart<TransformRequest>,
) -> Result<impl IntoResponse, AppError> {
    let base_image = payload.image.borrow();
    let mut test = ImageReader::new(Cursor::new(payload.image.contents.clone()));
    let mimetype = base_image.metadata.content_type.as_ref();
    let unwraped_mimetype = mimetype.ok_or(AppError::MissingMimeType)?;
    test.set_format(
        ImageFormat::from_mime_type(unwraped_mimetype)
            .ok_or(AppError::InvalidMimeType(unwraped_mimetype.into()))?,
    );
    let image = test
        .decode()
        .map_err(|err| AppError::DecodingFailure(err))?;

    let result: Result<DynamicImage, AppError> = payload
        .layers
        .iter()
        .map(prepare_layers(image.width(), image.height()))
        .try_fold(image.clone(), |mut acc, layer| {
            overlay(&mut acc, &layer?, 0, 0);
            return Ok(acc);
        });

    let mut default = vec![];
    let encoder = JpegEncoder::new(&mut default);
    result?
        .write_with_encoder(encoder)
        .map_err(|_| AppError::EncodingFailure)?;

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "image/jpeg".parse().unwrap());
    return Ok((headers, default));
}
