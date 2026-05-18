//! Standalone e2e — exercises `ThumbCache` against a counted axum endpoint.
//! Runs on every platform (no sqld dependency).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

fn tiny_jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    bytes
}

async fn serve(jpeg: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    use axum::{routing::get, Router};
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let jpeg_clone = jpeg.clone();
    let app = Router::new().route(
        "/thumbs/{hash}",
        get(move || {
            let counter = counter_clone.clone();
            let jpeg = jpeg_clone.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], jpeg)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), counter)
}

#[tokio::test]
async fn two_gets_one_http_request() {
    let tmp = TempDir::new().unwrap();
    let (url, counter) = serve(tiny_jpeg()).await;
    let cache = shoebox_client::thumb_cache::ThumbCache::new(
        reqwest::Client::new(),
        url,
        tmp.path().to_path_buf(),
    )
    .unwrap();
    assert!(cache.get("abc").await.is_ok());
    assert!(cache.get("abc").await.is_ok());
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
