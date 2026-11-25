//! Descarga a la caché: verificación sha512, reanudación y reintentos.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mergeon_updater::testing::{FakeResponse, FakeTransport};
use mergeon_updater::{
    DownloadRequest, DownloadedUpdate, Downloader, TransportError, UpdaterError, sha512_base64,
    sha512_base64_of_file, sha512_matches, verify_sha512,
};

use common::{fast_download_settings, payload};

const FILE_NAME: &str = "Mergeon-0.9.1.AppImage";
const URL: &str = "https://updates.example.com/app/Mergeon-0.9.1.AppImage";

fn request_for(data: &[u8]) -> DownloadRequest {
    DownloadRequest {
        url: URL.to_string(),
        file_name: FILE_NAME.to_string(),
        sha512: sha512_base64(data),
        size: Some(data.len() as u64),
        version: "0.9.1".to_string(),
    }
}

fn downloader(transport: &Arc<FakeTransport>, cache: &Path) -> Downloader {
    Downloader::new(transport.clone(), cache, fast_download_settings())
}

/// Descarga registrando el progreso emitido.
fn run(
    downloader: &Downloader,
    request: &DownloadRequest,
) -> (Result<DownloadedUpdate, UpdaterError>, Vec<f64>) {
    let mut progress = Vec::new();
    let result = downloader.download(
        request,
        &mut |percent| progress.push(percent),
        &AtomicBool::new(false),
    );
    (result, progress)
}

fn range_headers(transport: &FakeTransport) -> Vec<Option<String>> {
    transport
        .requests()
        .iter()
        .map(|request| request.header("range").map(str::to_string))
        .collect()
}

#[test]
fn hashes_like_electron_builder() {
    // sha512("abc") en base64, el formato de `latest.yml`.
    const ABC: &str =
        "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==";
    assert_eq!(sha512_base64(b"abc"), ABC);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abc.bin");
    fs::write(&path, b"abc").unwrap();
    assert_eq!(sha512_base64_of_file(&path).unwrap(), ABC);
    assert_eq!(verify_sha512(&path, ABC), Ok(()));
    // Con espacios alrededor o sin el relleno sigue siendo el mismo hash.
    assert_eq!(verify_sha512(&path, &format!("  {ABC}\n")), Ok(()));
    assert!(sha512_matches(ABC.trim_end_matches('='), ABC));

    let other = sha512_base64(b"abd");
    assert!(!sha512_matches(&other, ABC));
    assert!(!sha512_matches("not base64 !!", ABC));
    assert_eq!(
        verify_sha512(&path, &other),
        Err(UpdaterError::ChecksumMismatch {
            expected: other,
            actual: ABC.to_string(),
        })
    );
    assert!(matches!(
        verify_sha512(&dir.path().join("missing.bin"), ABC),
        Err(UpdaterError::Io { .. })
    ));
}

#[test]
fn downloads_the_artifact_and_reports_progress() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(300_000);
    let transport = Arc::new(FakeTransport::new());
    transport.serve_file(FILE_NAME, data.clone());
    let request = request_for(&data);

    let (result, progress) = run(&downloader(&transport, cache.path()), &request);

    let update = result.unwrap();
    assert_eq!(update.version, "0.9.1");
    assert_eq!(update.file_name, FILE_NAME);
    assert_eq!(update.size, data.len() as u64);
    assert_eq!(update.sha512, request.sha512);
    assert_eq!(update.path, cache.path().join("pending").join(FILE_NAME));
    assert_eq!(fs::read(&update.path).unwrap(), data);
    // No queda el archivo parcial.
    assert!(
        !cache
            .path()
            .join("pending")
            .join(format!("{FILE_NAME}.part"))
            .exists()
    );

    assert_eq!(progress.first(), Some(&0.0));
    assert_eq!(progress.last(), Some(&100.0));
    assert!(
        progress.windows(2).all(|pair| pair[0] < pair[1]),
        "{progress:?}"
    );
    assert!(progress.len() > 2 && progress.len() <= 101);
    assert_eq!(range_headers(&transport), [None]);
}

#[test]
fn resumes_after_the_connection_drops_mid_download() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(200_000);
    let transport = Arc::new(FakeTransport::new());
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let data = data.clone();
        let calls = calls.clone();
        transport.route(FILE_NAME, move |request| {
            let response = FakeResponse::file(request, &data);
            // La primera conexión se corta a los 70 000 bytes.
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(response.failing_after(70_000))
            } else {
                Ok(response)
            }
        });
    }

    let (result, progress) = run(&downloader(&transport, cache.path()), &request_for(&data));

    let update = result.unwrap();
    assert_eq!(fs::read(&update.path).unwrap(), data);
    assert_eq!(
        range_headers(&transport),
        [None, Some("bytes=70000-".to_string())]
    );
    // El segundo intento arranca donde quedó el primero, no desde cero.
    assert_eq!(progress.last(), Some(&100.0));
    assert!(progress.iter().filter(|percent| **percent == 0.0).count() == 1);
}

#[test]
fn resumes_a_partial_download_left_by_a_previous_run() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(120_000);
    let request = request_for(&data);

    // Primera ejecución: la red se cae a mitad y se agotan los intentos.
    let failing = Arc::new(FakeTransport::new());
    {
        let data = data.clone();
        failing.route(FILE_NAME, move |request| {
            Ok(FakeResponse::file(request, &data).failing_after(10_000))
        });
    }
    let (result, _) = run(&downloader(&failing, cache.path()), &request);
    assert!(matches!(result, Err(UpdaterError::Network { .. })));
    assert_eq!(failing.requests().len(), 3);
    let part = cache
        .path()
        .join("pending")
        .join(format!("{FILE_NAME}.part"));
    assert_eq!(fs::metadata(&part).unwrap().len(), 30_000);

    // Segunda ejecución (otro proceso): continúa desde el byte 30 000.
    let healthy = Arc::new(FakeTransport::new());
    healthy.serve_file(FILE_NAME, data.clone());
    let (result, progress) = run(&downloader(&healthy, cache.path()), &request);

    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(range_headers(&healthy), [Some("bytes=30000-".to_string())]);
    assert_eq!(progress.first(), Some(&25.0));
    assert!(!part.exists());
}

#[test]
fn starts_over_when_the_server_ignores_the_range() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(50_000);
    let request = request_for(&data);
    let pending = cache.path().join("pending");

    let failing = Arc::new(FakeTransport::new());
    {
        let data = data.clone();
        failing.route(FILE_NAME, move |_| {
            Ok(FakeResponse::ok(data.clone()).failing_after(20_000))
        });
    }
    let mut settings = fast_download_settings();
    settings.attempts = 1;
    let single_attempt = Downloader::new(failing.clone(), cache.path(), settings);
    assert!(run(&single_attempt, &request).0.is_err());
    assert_eq!(
        fs::metadata(pending.join(format!("{FILE_NAME}.part")))
            .unwrap()
            .len(),
        20_000
    );

    // Este servidor responde 200 con el archivo entero aunque se le pida un rango.
    let no_ranges = Arc::new(FakeTransport::new());
    no_ranges.serve(FILE_NAME, data.clone());
    let (result, _) = run(&downloader(&no_ranges, cache.path()), &request);

    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(
        range_headers(&no_ranges),
        [Some("bytes=20000-".to_string())]
    );
}

#[test]
fn discards_a_partial_download_of_another_artifact() {
    let cache = tempfile::tempdir().unwrap();
    let old = payload(40_000);
    let old_request = DownloadRequest {
        version: "0.9.0".to_string(),
        ..request_for(&old)
    };

    let failing = Arc::new(FakeTransport::new());
    {
        let old = old.clone();
        failing.route(FILE_NAME, move |request| {
            Ok(FakeResponse::file(request, &old).failing_after(5_000))
        });
    }
    assert!(
        run(&downloader(&failing, cache.path()), &old_request)
            .0
            .is_err()
    );

    // Mismo nombre de archivo, otro contenido: el `.part` viejo no vale.
    let data: Vec<u8> = payload(40_000).iter().map(|byte| byte ^ 0xff).collect();
    let healthy = Arc::new(FakeTransport::new());
    healthy.serve_file(FILE_NAME, data.clone());
    let (result, _) = run(&downloader(&healthy, cache.path()), &request_for(&data));

    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(range_headers(&healthy), [None]);
}

#[test]
fn a_corrupt_partial_download_gets_one_clean_retry() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(60_000);
    let request = request_for(&data);
    let pending = cache.path().join("pending");
    let part = pending.join(format!("{FILE_NAME}.part"));

    // Un `.part` del mismo artefacto (mismo sha512 anotado) pero con basura.
    let failing = Arc::new(FakeTransport::new());
    failing.fail(
        FILE_NAME,
        TransportError::Network("connection refused".to_string()),
    );
    assert!(
        run(&downloader(&failing, cache.path()), &request)
            .0
            .is_err()
    );
    fs::write(&part, vec![0u8; 25_000]).unwrap();

    let healthy = Arc::new(FakeTransport::new());
    healthy.serve_file(FILE_NAME, data.clone());
    let (result, _) = run(&downloader(&healthy, cache.path()), &request);

    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(
        range_headers(&healthy),
        [Some("bytes=25000-".to_string()), None]
    );
}

#[test]
fn rejects_a_download_whose_sha512_does_not_match() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(10_000);
    let mut tampered = data.clone();
    tampered[1234] ^= 0x01;
    let transport = Arc::new(FakeTransport::new());
    transport.serve_file(FILE_NAME, tampered.clone());
    let request = request_for(&data);

    let (result, _) = run(&downloader(&transport, cache.path()), &request);

    assert_eq!(
        result,
        Err(UpdaterError::ChecksumMismatch {
            expected: request.sha512.clone(),
            actual: sha512_base64(&tampered),
        })
    );
    // Una descarga limpia con hash incorrecto no se reintenta ni se conserva.
    assert_eq!(transport.requests().len(), 1);
    let pending = cache.path().join("pending");
    assert!(!pending.join(FILE_NAME).exists());
    assert!(!pending.join(format!("{FILE_NAME}.part")).exists());
}

#[test]
fn gives_up_after_the_configured_attempts_when_the_network_is_down() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(1_000);
    let transport = Arc::new(FakeTransport::new());
    transport.fail(
        FILE_NAME,
        TransportError::Network("io: connection refused".to_string()),
    );

    let (result, progress) = run(&downloader(&transport, cache.path()), &request_for(&data));

    assert_eq!(
        result,
        Err(UpdaterError::Network {
            url: URL.to_string(),
            detail: "io: connection refused".to_string(),
        })
    );
    assert_eq!(transport.requests().len(), 3);
    assert!(progress.is_empty());
}

#[test]
fn retries_server_errors_but_not_client_errors() {
    let data = payload(2_000);

    // 503 dos veces y luego el archivo: se recupera.
    let cache = tempfile::tempdir().unwrap();
    let flaky = Arc::new(FakeTransport::new());
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let data = data.clone();
        let calls = calls.clone();
        flaky.route(FILE_NAME, move |request| {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                Ok(FakeResponse::status(503))
            } else {
                Ok(FakeResponse::file(request, &data))
            }
        });
    }
    let (result, _) = run(&downloader(&flaky, cache.path()), &request_for(&data));
    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(flaky.requests().len(), 3);

    // 404: el archivo no está y no va a aparecer por insistir.
    let cache = tempfile::tempdir().unwrap();
    let missing = Arc::new(FakeTransport::new());
    let (result, _) = run(&downloader(&missing, cache.path()), &request_for(&data));
    assert_eq!(
        result,
        Err(UpdaterError::HttpStatus {
            status: 404,
            url: URL.to_string(),
        })
    );
    assert_eq!(missing.requests().len(), 1);

    let cache = tempfile::tempdir().unwrap();
    let forbidden = Arc::new(FakeTransport::new());
    forbidden.serve_status(FILE_NAME, 403);
    let (result, _) = run(&downloader(&forbidden, cache.path()), &request_for(&data));
    assert!(matches!(
        result,
        Err(UpdaterError::HttpStatus { status: 403, .. })
    ));
    assert_eq!(forbidden.requests().len(), 1);
}

#[test]
fn restarts_when_the_partial_file_is_longer_than_the_server_copy() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(8_000);
    // El manifiesto no trae tamaño: no se puede saber de antemano que el
    // `.part` sobra, y es el 416 del servidor el que lo delata.
    let request = DownloadRequest {
        size: None,
        ..request_for(&data)
    };
    let pending = cache.path().join("pending");

    let failing = Arc::new(FakeTransport::new());
    failing.fail(FILE_NAME, TransportError::Timeout);
    assert!(
        run(&downloader(&failing, cache.path()), &request)
            .0
            .is_err()
    );
    fs::write(pending.join(format!("{FILE_NAME}.part")), vec![7u8; 9_000]).unwrap();

    let healthy = Arc::new(FakeTransport::new());
    healthy.serve_file(FILE_NAME, data.clone());
    let (result, _) = run(&downloader(&healthy, cache.path()), &request);

    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(
        range_headers(&healthy),
        [Some("bytes=9000-".to_string()), None]
    );
}

#[test]
fn detects_a_body_that_does_not_match_the_announced_size() {
    let data = payload(5_000);

    // Más bytes de los anunciados: no es el archivo del manifiesto.
    let cache = tempfile::tempdir().unwrap();
    let longer = Arc::new(FakeTransport::new());
    let mut extended = data.clone();
    extended.extend_from_slice(b"extra");
    longer.serve(FILE_NAME, extended);
    let (result, _) = run(&downloader(&longer, cache.path()), &request_for(&data));
    assert_eq!(
        result,
        Err(UpdaterError::SizeMismatch {
            expected: 5_000,
            actual: 5_005,
        })
    );
    assert_eq!(longer.requests().len(), 1);

    // Menos bytes sin error de lectura: se trata como corte y se reanuda.
    let cache = tempfile::tempdir().unwrap();
    let truncating = Arc::new(FakeTransport::new());
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let data = data.clone();
        let calls = calls.clone();
        truncating.route(FILE_NAME, move |request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(FakeResponse::ok(data[..2_000].to_vec()))
            } else {
                Ok(FakeResponse::file(request, &data))
            }
        });
    }
    let (result, _) = run(&downloader(&truncating, cache.path()), &request_for(&data));
    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(
        range_headers(&truncating),
        [None, Some("bytes=2000-".to_string())]
    );
}

#[test]
fn reuses_a_verified_download_and_replaces_a_damaged_one() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(30_000);
    let request = request_for(&data);
    let transport = Arc::new(FakeTransport::new());
    transport.serve_file(FILE_NAME, data.clone());
    let downloader = downloader(&transport, cache.path());

    let first = run(&downloader, &request).0.unwrap();
    assert_eq!(transport.requests().len(), 1);

    // Ya está en caché y verifica: ni una petición más.
    let (second, progress) = run(&downloader, &request);
    assert_eq!(second.unwrap(), first);
    assert_eq!(transport.requests().len(), 1);
    assert!(progress.is_empty());

    // Si alguien lo toca, deja de valer y se baja de nuevo.
    fs::write(&first.path, b"damaged").unwrap();
    let third = run(&downloader, &request).0.unwrap();
    assert_eq!(fs::read(&third.path).unwrap(), data);
    assert_eq!(transport.requests().len(), 2);
}

#[test]
fn clear_removes_everything_downloaded() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(1_000);
    let transport = Arc::new(FakeTransport::new());
    transport.serve_file(FILE_NAME, data.clone());
    let downloader = downloader(&transport, cache.path());
    let update = run(&downloader, &request_for(&data)).0.unwrap();
    assert_eq!(downloader.pending_dir(), cache.path().join("pending"));

    downloader.clear();

    assert!(!update.path.exists());
    assert!(!downloader.pending_dir().exists());
    // Limpiar una caché vacía no es un error.
    downloader.clear();
}

#[test]
fn stops_when_cancelled_and_keeps_what_it_had() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(400_000);
    let transport = Arc::new(FakeTransport::new());
    transport.serve_file(FILE_NAME, data.clone());
    let downloader = downloader(&transport, cache.path());
    let cancel = AtomicBool::new(false);

    // Se cancela desde el propio callback de progreso, a mitad de descarga.
    let result = downloader.download(
        &request_for(&data),
        &mut |percent| {
            if percent >= 40.0 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        &cancel,
    );

    assert_eq!(result, Err(UpdaterError::Cancelled));
    assert_eq!(transport.requests().len(), 1);
    let part = cache
        .path()
        .join("pending")
        .join(format!("{FILE_NAME}.part"));
    let kept = fs::metadata(&part).unwrap().len();
    assert!(kept > 0 && kept < data.len() as u64, "{kept}");

    // Ya cancelado, ni lo intenta.
    assert_eq!(
        downloader.download(&request_for(&data), &mut |_| {}, &cancel),
        Err(UpdaterError::Cancelled)
    );
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn reports_progress_without_a_known_size() {
    let cache = tempfile::tempdir().unwrap();
    let data = payload(90_000);
    let transport = Arc::new(FakeTransport::new());
    // Ni el manifiesto ni el servidor dicen cuánto pesa.
    {
        let data = data.clone();
        transport.route(FILE_NAME, move |_| {
            let mut response = FakeResponse::ok(data.clone());
            response.headers.clear();
            Ok(response)
        });
    }
    let request = DownloadRequest {
        size: None,
        ..request_for(&data)
    };

    let (result, progress) = run(&downloader(&transport, cache.path()), &request);

    assert_eq!(fs::read(result.unwrap().path).unwrap(), data);
    assert_eq!(progress, [0.0]);

    // Con `Content-Length` basta para dar porcentajes.
    let cache = tempfile::tempdir().unwrap();
    let with_length = Arc::new(FakeTransport::new());
    with_length.serve(FILE_NAME, data.clone());
    let (_, progress) = run(&downloader(&with_length, cache.path()), &request);
    assert_eq!(progress.last(), Some(&100.0));
}
