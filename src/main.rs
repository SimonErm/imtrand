use std::{borrow::Borrow, io::Cursor};

use axum::{body::Bytes, http::HeaderMap, response::IntoResponse, routing::post, Router};
use axum_typed_multipart::{FieldData, TryFromMultipart, TypedMultipart};
use imageproc::image::{
    codecs::jpeg::JpegEncoder,
    imageops::{overlay, FilterType},
    ImageFormat, ImageReader,
};
use resvg::{
    tiny_skia::{self, IntSize},
    usvg::{self},
};
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
async fn create_document(payload: TypedMultipart<TransformRequest>) -> impl IntoResponse {
    let base_image = payload.image.borrow();
    let mut test = ImageReader::new(Cursor::new(payload.image.contents.clone()));
    let mimetype = base_image.metadata.content_type.as_ref();
    test.set_format(
        ImageFormat::from_mime_type(mimetype.expect("msg")).expect("Invalid image type"),
    );
    let image = test.decode().expect("Failed to decode image");

    let result = payload
        .layers
        .iter()
        .map(|layer| {
            if layer.metadata.content_type.as_ref().unwrap() == "image/svg+xml" {
                let tree = {
                    let mut opt = usvg::Options::default();
                    opt.fontdb_mut().load_system_fonts();

                    let svg_data = layer.contents.clone();
                    usvg::Tree::from_data(&svg_data, &opt).unwrap()
                };

                //let render_ts = tiny_skia::Transform::from_scale(zoom, zoom);
                let original_size = tree.size().to_int_size();
                let pixmap_size = tree
                    .size()
                    .to_int_size()
                    .scale_to(IntSize::from_wh(image.width(), image.height()).unwrap());
                let mut pixmap =
                    tiny_skia::Pixmap::new(pixmap_size.width(), pixmap_size.height()).unwrap();
                let transfrom = tiny_skia::Transform::from_scale(
                    image.width() as f32 / original_size.width() as f32,
                    image.height() as f32 / original_size.height() as f32,
                );

                resvg::render(&tree, transfrom, &mut pixmap.as_mut());

                let rgba = pixmap.encode_png().unwrap();
                let mut overlay_reader =
                    ImageReader::new(Cursor::new(Bytes::from_iter(rgba.into_iter())));
                overlay_reader.set_format(ImageFormat::Png);
                let mut overlay_image = overlay_reader.decode().expect("Failed to decode image");
                overlay_image =
                    overlay_image.resize(image.width(), image.height(), FilterType::Nearest);
                return overlay_image;
            } else {
                let mut overlay_reader = ImageReader::new(Cursor::new(layer.contents.clone()));
                let mimetype = layer.metadata.content_type.as_ref();
                overlay_reader.set_format(
                    ImageFormat::from_mime_type(mimetype.expect("msg"))
                        .expect("Invalid image type"),
                );
                let mut overlay_image = overlay_reader.decode().expect("Failed to decode image");
                overlay_image =
                    overlay_image.resize(image.width(), image.height(), FilterType::Nearest);
                return overlay_image;
            }
        })
        .fold(image.clone(), |mut acc, layer| {
            overlay(&mut acc, &layer, 0, 0);
            return acc;
        });

    let mut default = vec![];
    let encoder = JpegEncoder::new(&mut default);
    result.write_with_encoder(encoder).expect("msg");

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "image/jpeg".parse().unwrap());
    return (headers, default);
}
