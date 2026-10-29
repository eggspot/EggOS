use super::*;
use crate::fake::{Fake, FakePage};
use eggos_net::Ipv4Address;

const HOST: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

fn page(port: u16, target: &str, response: &[u8]) -> FakePage {
    FakePage {
        address: HOST,
        port,
        target: target.as_bytes().to_vec(),
        response: response.to_vec(),
    }
}

fn network() -> Fake {
    let mut fake = Fake::default().with_directory("/home", 0).with_file(
        "/etc/resolv.conf",
        0o644,
        b"nameserver 10.0.2.3\n",
        0,
    );
    fake.network
        .dns_servers
        .push((Ipv4Address::new(10, 0, 2, 3), 53));
    fake.network.names.push((b"files.test".to_vec(), HOST));
    fake.network.reachable.push(HOST);
    fake.network.pages.push(page(
        8080,
        "/hello.txt",
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, EggOS!",
    ));
    fake.network.pages.push(page(
        80,
        "/chunked",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nHello\r\n2\r\n, \r\n6\r\nchunks\r\n0\r\n\r\n",
    ));
    fake.network.pages.push(page(
        80,
        "/old",
        b"HTTP/1.1 301 Moved Permanently\r\nLocation: /hello\r\nContent-Length: 0\r\n\r\n",
    ));
    fake.network.pages.push(page(
        80,
        "/hello",
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.0 200 OK\r\n\r\nuntil the end",
    ));
    fake.network.pages.push(page(
        80,
        "/loop",
        b"HTTP/1.1 302 Found\r\nLocation: http://files.test/loop\r\n\r\n",
    ));
    fake.network.pages.push(page(
        80,
        "/cut",
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nonly this",
    ));
    fake
}

fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
    let mut output = Output::new(fake, b"fetch");
    let status = fetch(&mut output, arguments);
    let status = output.finish(status);
    let fake = output.system;
    assert_eq!(fake.open_count(), 0, "every file is closed");
    assert!(
        fake.network.streams.iter().all(|stream| !stream.open),
        "every socket is closed"
    );
    let (out, errors) = (fake.output(), fake.errors());
    (status, out, errors, fake)
}

#[test]
fn a_file_is_saved_and_its_size_and_status_said() {
    let (status, out, errors, fake) = run(
        network(),
        &[c"-o", c"/home/hello.txt", c"http://10.0.2.2:8080/hello.txt"],
    );
    assert_eq!((status, errors.as_str()), (0, ""));
    assert_eq!(out, "Saved 13 bytes to /home/hello.txt (HTTP 200 OK)\n");
    assert_eq!(fake.contents("/home/hello.txt"), b"Hello, EggOS!");
    assert_eq!(
        fake.network.requests,
        [
            b"GET /hello.txt HTTP/1.1\r\nHost: 10.0.2.2:8080\r\nUser-Agent: EggOS-fetch\r\n\
           Accept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n"
                .to_vec()
        ]
    );
}

#[test]
fn without_a_file_the_body_goes_to_standard_output_decoded() {
    let (status, out, _, _) = run(network(), &[c"http://files.test/chunked"]);
    assert_eq!((status, out.as_str()), (0, "Hello, chunks"));
    let (status, out, _, fake) = run(
        network(),
        &[c"-q", c"http://files.test/chunked", c"/home/c"],
    );
    assert_eq!((status, out.as_str()), (0, ""));
    assert_eq!(fake.contents("/home/c"), b"Hello, chunks");
}

#[test]
fn a_redirect_is_followed_and_an_interim_answer_skipped() {
    let (status, out, errors, fake) = run(network(), &[c"http://files.test/old"]);
    assert_eq!(
        (status, out.as_str(), errors.as_str()),
        (0, "until the end", "")
    );
    assert_eq!(fake.network.requests.len(), 2);
    assert!(fake.network.requests[1].starts_with(b"GET /hello HTTP/1.1\r\nHost: files.test\r\n"));
    let (status, _, errors, fake) = run(network(), &[c"http://files.test/loop"]);
    assert_eq!(status, 1);
    assert_eq!(
        errors,
        "fetch: http://files.test/loop: more than 5 redirects\n"
    );
    assert_eq!(fake.network.requests.len(), 6);
}

#[test]
fn failures_say_what_went_wrong() {
    let cases: [(&[&CStr], &str); 7] = [
        (
            &[c"http://files.test/missing"],
            "fetch: files.test:80/missing: 404 Not Found\n",
        ),
        (
            &[c"http://10.0.2.2:9/"],
            "fetch: 10.0.2.2:9: Connection refused\n",
        ),
        (
            &[c"http://192.0.2.1/"],
            "fetch: 192.0.2.1:80: Connection timed out\n",
        ),
        (
            &[c"https://files.test/"],
            "fetch: files.test:443: Connection refused\n",
        ),
        (
            &[c"http://files.test/cut"],
            "fetch: files.test:80: the connection closed before the whole file came\n",
        ),
        (&[], "fetch: usage: fetch [-q] [-o <file>] <url> [<file>]\n"),
        (&[c"-o"], "fetch: option requires an argument -- 'o'\n"),
    ];
    for (arguments, expected) in cases {
        let (status, _, errors, _) = run(network(), arguments);
        assert_eq!((status, errors.as_str()), (1, expected), "{arguments:?}");
    }
    let (status, _, errors, _) = run(network(), &[c"-x", c"http://files.test/"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "fetch: invalid option -- 'x'\n")
    );
}

#[test]
fn https_runs_tls_and_says_why_the_server_refused() {
    let mut fake = network();
    fake.network
        .raw_answers
        .push(((HOST, 443), vec![21, 3, 3, 0, 2, 2, 40]));
    let (status, _, errors, fake) = run(fake, &[c"https://files.test/"]);
    assert_eq!(
        (status, errors.as_str()),
        (
            1,
            "fetch: files.test:443: the server ended the TLS handshake with an alert (40)\n"
        )
    );
    let hello = &fake.network.streams[0].request;
    assert_eq!(hello[0], 22);
    assert!(hello.windows(10).any(|window| window == b"files.test"));
}
