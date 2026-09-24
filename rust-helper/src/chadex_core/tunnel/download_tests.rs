use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn download_limit_applies_to_declared_and_chunked_bodies() {
    for (wire, accepted) in [
        ("HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n12345678", true),
        ("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n123456789", false),
        ("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n1234\r\n5\r\n56789\r\n0\r\n\r\n", false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            stream.read(&mut buffer).await.unwrap();
            stream.write_all(wire.as_bytes()).await.unwrap();
        });
        let response = Client::builder().no_proxy().build().unwrap().get(url).send().await.unwrap();
        let result = bounded_download(response, 8).await;
        assert_eq!(result.is_ok(), accepted);
        if accepted { assert_eq!(result.unwrap(), b"12345678"); }
        server.await.unwrap();
    }
}
