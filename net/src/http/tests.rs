use super::*;

fn decode_all(body: &mut Body, input: &[u8]) -> Result<(Vec<u8>, usize), BodyError> {
    let mut data = Vec::new();
    let mut used = 0;
    while used < input.len() && !body.is_done() {
        let decoded = body.decode(&input[used..])?;
        data.extend_from_slice(decoded.data);
        used += decoded.used;
        if decoded.used == 0 {
            break;
        }
    }
    Ok((data, used))
}

#[test]
fn urls_parse_with_their_port_and_target_and_bad_ones_are_refused() {
    assert_eq!(
        parse_url(b"http://example.com"),
        Ok(Url {
            host: b"example.com",
            port: 80,
            target: b"/"
        })
    );
    assert_eq!(
        parse_url(b"HTTP://10.0.2.2:8080/files/a.txt?x=1#top"),
        Ok(Url {
            host: b"10.0.2.2",
            port: 8080,
            target: b"/files/a.txt?x=1"
        })
    );
    assert_eq!(parse_url(b"http://h:/x").unwrap().port, 80);
    assert_eq!(parse_url(b"https://example.com/"), Err(UrlError::Https));
    assert_eq!(parse_url(b"ftp://example.com/"), Err(UrlError::NotHttp));
    assert_eq!(parse_url(b"example.com"), Err(UrlError::NotHttp));
    assert_eq!(parse_url(b"http:///x"), Err(UrlError::BadHost));
    assert_eq!(parse_url(b"http://user@h/"), Err(UrlError::BadHost));
    assert_eq!(parse_url(b"http://-h/"), Err(UrlError::BadHost));
    assert_eq!(parse_url(b"http://h:0/"), Err(UrlError::BadPort));
    assert_eq!(parse_url(b"http://h:65536/"), Err(UrlError::BadPort));
    assert_eq!(parse_url(b"http://h:8x/"), Err(UrlError::BadPort));
    assert_eq!(parse_url(b"http://h/a b"), Err(UrlError::BadCharacter));
    assert!(!UrlError::Https.description().is_empty());
}

#[test]
fn a_redirect_is_a_whole_url_or_a_path_on_the_same_host() {
    assert_eq!(redirect(b"/next#x"), Ok(Redirect::Path(b"/next")));
    assert_eq!(
        redirect(b"http://other:81/y"),
        Ok(Redirect::Absolute(Url {
            host: b"other",
            port: 81,
            target: b"/y"
        }))
    );
    assert_eq!(redirect(b"//other/y"), Err(UrlError::NotHttp));
    assert_eq!(redirect(b"relative"), Err(UrlError::NotHttp));
    assert_eq!(redirect(b"https://h/"), Err(UrlError::Https));
}

#[test]
fn the_request_names_the_target_host_and_port_and_asks_to_close() {
    let url = parse_url(b"http://10.0.2.2:8080/a.txt").unwrap();
    let mut out = [0u8; 256];
    let length = write_request(&mut out, &url, b"EggOS-fetch/1").unwrap();
    assert_eq!(
        &out[..length],
        b"GET /a.txt HTTP/1.1\r\nHost: 10.0.2.2:8080\r\nUser-Agent: EggOS-fetch/1\r\n\
          Accept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n"
    );
    let plain = parse_url(b"http://example.com").unwrap();
    let length = write_request(&mut out, &plain, b"x").unwrap();
    assert!(out[..length].starts_with(b"GET / HTTP/1.1\r\nHost: example.com\r\n"));
    assert_eq!(write_request(&mut out[..20], &plain, b"x"), None);
}

#[test]
fn the_head_ends_at_the_first_empty_line_with_or_without_carriage_returns() {
    assert_eq!(head_end(b"HTTP/1.1 200 OK\r\nA: b\r\n\r\nbody"), Some(25));
    assert_eq!(head_end(b"HTTP/1.1 200 OK\nA: b\n\nbody"), Some(22));
    assert_eq!(head_end(b"HTTP/1.1 200 OK\r\nA: b\r\n"), None);
}

#[test]
fn a_head_gives_the_status_the_framing_and_the_location() {
    let head =
        parse_head(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\ncontent-length:  12 \r\n\r\n")
            .unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(head.reason, b"OK");
    assert_eq!(head.framing, Framing::Length(12));
    assert_eq!(head.content_type, Some(&b"text/plain"[..]));
    let chunked =
        parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n")
            .unwrap();
    assert_eq!(chunked.framing, Framing::Chunked);
    let moved = parse_head(b"HTTP/1.0 301 Moved Permanently\r\nLocation: /new\r\n\r\n").unwrap();
    assert!(moved.is_redirect());
    assert_eq!(moved.location, Some(&b"/new"[..]));
    assert_eq!(moved.framing, Framing::UntilClose);
    assert_eq!(
        parse_head(b"HTTP/1.1 304 Not Modified\r\n\r\n")
            .unwrap()
            .framing,
        Framing::Length(0)
    );
    assert!(
        parse_head(b"HTTP/1.1 100 Continue\r\n\r\n")
            .unwrap()
            .is_interim()
    );
    assert_eq!(parse_head(b"HTTP/1.1 204\r\n\r\n").unwrap().reason, b"");
    assert_eq!(
        parse_head(b"ICY 200 OK\r\n\r\n"),
        Err(HeadError::BadStatusLine)
    );
    assert_eq!(
        parse_head(b"HTTP/1.1 2x0 OK\r\n\r\n"),
        Err(HeadError::BadStatusLine)
    );
    assert_eq!(
        parse_head(b"HTTP/1.1 200 OK\r\nNoColon\r\n\r\n"),
        Err(HeadError::BadHeader)
    );
    assert_eq!(
        parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n"),
        Err(HeadError::BadLength)
    );
    assert_eq!(
        parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n"),
        Err(HeadError::BadLength)
    );
    assert_eq!(
        parse_head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n"),
        Err(HeadError::UnsupportedEncoding)
    );
}

#[test]
fn a_body_of_known_length_stops_at_its_end() {
    let mut body = Body::new(Framing::Length(5));
    let (data, used) = decode_all(&mut body, b"helloEXTRA").unwrap();
    assert_eq!(data, b"hello");
    assert_eq!(used, 5);
    assert!(body.is_done());
    assert_eq!(body.finish(), Ok(()));
    let mut short = Body::new(Framing::Length(5));
    decode_all(&mut short, b"hel").unwrap();
    assert_eq!(short.finish(), Err(BodyError::Truncated));
    let mut open = Body::new(Framing::UntilClose);
    assert_eq!(decode_all(&mut open, b"all of it").unwrap().0, b"all of it");
    assert_eq!(open.finish(), Ok(()));
}

#[test]
fn a_chunked_body_decodes_whole_and_byte_by_byte() {
    let encoded =
        b"5;name=value\r\nhello\r\n7\r\n, world\r\nA\r\n0123456789\r\n0\r\nTrailer: x\r\n\r\nNEXT";
    let mut body = Body::new(Framing::Chunked);
    let (data, used) = decode_all(&mut body, encoded).unwrap();
    assert_eq!(data, b"hello, world0123456789");
    assert_eq!(used, encoded.len() - 4);
    assert!(body.is_done());
    let mut piecewise = Body::new(Framing::Chunked);
    let mut data = Vec::new();
    for byte in encoded.chunks(1) {
        if piecewise.is_done() {
            break;
        }
        data.extend_from_slice(decode_all(&mut piecewise, byte).unwrap().0.as_slice());
    }
    assert_eq!(data, b"hello, world0123456789");
    assert_eq!(piecewise.finish(), Ok(()));
}

#[test]
fn a_malformed_or_cut_chunked_body_is_refused() {
    let mut bad_size = Body::new(Framing::Chunked);
    assert_eq!(
        decode_all(&mut bad_size, b"zz\r\n"),
        Err(BodyError::BadChunk)
    );
    let mut no_size = Body::new(Framing::Chunked);
    assert_eq!(decode_all(&mut no_size, b"\r\n"), Err(BodyError::BadChunk));
    let mut no_crlf = Body::new(Framing::Chunked);
    assert_eq!(
        decode_all(&mut no_crlf, b"1\r\naX"),
        Err(BodyError::BadChunk)
    );
    let mut huge = Body::new(Framing::Chunked);
    assert_eq!(
        decode_all(&mut huge, b"1000000000000000\r\n"),
        Err(BodyError::BadChunk)
    );
    let mut cut = Body::new(Framing::Chunked);
    decode_all(&mut cut, b"5\r\nhel").unwrap();
    assert_eq!(cut.finish(), Err(BodyError::Truncated));
}
