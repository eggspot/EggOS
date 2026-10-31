use super::*;

fn complete(text: &[u8]) -> (Request<'_>, usize) {
    match parse(text, 1 << 20) {
        Parsed::Complete(request, used) => (request, used),
        other => panic!("not complete: {other:?}"),
    }
}

const MCP_POST: &[u8] = b"POST /mcp?x=1 HTTP/1.1\r\nHost: eggos.local:8443\r\n\
Authorization: Bearer eggos_abc\r\nContent-Type: application/json; charset=utf-8\r\n\
Content-Length: 2\r\n\r\n{}";

#[test]
fn a_request_is_read_once_its_body_arrives_and_leaves_the_next_one() {
    for cut in 0..MCP_POST.len() {
        assert_eq!(parse(&MCP_POST[..cut], 1 << 20), Parsed::Incomplete);
    }
    let mut two = MCP_POST.to_vec();
    two.extend_from_slice(b"GET /mcp HTTP/1.0\r\n\r\n");
    let (request, used) = complete(&two);
    assert_eq!(used, MCP_POST.len());
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.path, b"/mcp");
    assert_eq!(request.body, b"{}");
    assert_eq!(request.host, Some(&b"eggos.local:8443"[..]));
    assert!(request.keep_alive);
    let (next, _) = complete(&two[used..]);
    assert_eq!(next.method, Method::Get);
    assert!(!next.keep_alive);
}

#[test]
fn requests_without_a_length_or_too_large_are_refused() {
    let cases: [(&[u8], Status); 6] = [
        (b"POST /mcp HTTP/1.1\r\n\r\n", Status::LengthRequired),
        (
            b"POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            Status::NotImplemented,
        ),
        (
            b"POST /mcp HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n",
            Status::PayloadTooLarge,
        ),
        (
            b"POST /mcp HTTP/1.1\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n",
            Status::BadRequest,
        ),
        (b"POST /mcp HTTP/2\r\n\r\n", Status::BadRequest),
        (
            b"POST /mcp HTTP/1.1\r\nno colon\r\n\r\n",
            Status::BadRequest,
        ),
    ];
    for (text, status) in cases {
        assert_eq!(parse(text, 1024), Parsed::Refused(status), "{text:?}");
    }
    let long = vec![b'a'; MAX_HEADER_BYTES + 1];
    assert_eq!(parse(&long, 1024), Parsed::Refused(Status::HeaderTooLarge));
}

#[test]
fn mcp_messages_need_a_bearer_token_json_and_their_own_origin() {
    let (request, _) = complete(MCP_POST);
    assert_eq!(
        route(&request),
        Route::Mcp(crate::pairing::token_hash(b"eggos_abc"), b"{}")
    );
    let lower = Request {
        authorization: Some(b"bearer   eggos_abc "),
        ..request
    };
    assert_eq!(route(&lower), route(&request));
    for refused in [
        Request {
            authorization: None,
            ..request
        },
        Request {
            authorization: Some(b"Basic abc"),
            ..request
        },
        Request {
            authorization: Some(b"Bearer "),
            ..request
        },
    ] {
        assert!(matches!(
            route(&refused),
            Route::Refused(Status::Unauthorized, _)
        ));
    }
    let plain = Request {
        content_type: Some(b"text/plain"),
        ..request
    };
    assert!(matches!(
        route(&plain),
        Route::Refused(Status::UnsupportedMediaType, _)
    ));
    let same_origin = Request {
        origin: Some(b"https://EGGOS.local:8443"),
        ..request
    };
    assert!(matches!(route(&same_origin), Route::Mcp(..)));
    for origin in [&b"https://evil.example"[..], b"http://eggos.local:8443"] {
        let other = Request {
            origin: Some(origin),
            ..request
        };
        assert!(matches!(
            route(&other),
            Route::Refused(Status::Forbidden, _)
        ));
    }
    let get = Request {
        method: Method::Get,
        ..request
    };
    assert!(matches!(
        route(&get),
        Route::Refused(Status::MethodNotAllowed, _)
    ));
    let elsewhere = Request {
        path: b"/",
        ..request
    };
    assert!(matches!(
        route(&elsewhere),
        Route::Refused(Status::NotFound, _)
    ));
    let pair = Request {
        path: b"/pair",
        authorization: None,
        content_type: None,
        body: b"{\"code\":\"ABCD-EFGH\"}",
        ..request
    };
    assert_eq!(route(&pair), Route::Pair(b"{\"code\":\"ABCD-EFGH\"}"));
}

#[test]
fn pairing_bodies_and_answers_are_json() {
    let mut code = [0u8; 32];
    assert_eq!(
        pair_code(b"{\"code\": \"abcd-efgh\"}", &mut code),
        Some(&b"abcd-efgh"[..])
    );
    assert_eq!(pair_code(b"{\"code\": 5}", &mut code), None);
    assert_eq!(pair_code(b"not json", &mut code), None);
    assert_eq!(pair_code(&[b'"'; 40], &mut code), None);
    let token = crate::pairing::token_from(&[0; 32]);
    let mut out = [0u8; 256];
    let length = pair_answer(b"phone", &token, &mut out);
    let text = core::str::from_utf8(&out[..length]).unwrap();
    assert_eq!(
        text,
        format!(
            "{{\"agent\":\"phone\",\"token\":\"{}\"}}",
            core::str::from_utf8(&token).unwrap()
        )
    );
    let length = error_body("no \"code\"", &mut out);
    assert_eq!(&out[..length], b"{\"error\":\"no \\\"code\\\"\"}");
}

#[test]
fn answers_carry_their_length_and_say_how_to_authenticate() {
    let mut out = [0u8; MAX_RESPONSE_HEADER_BYTES];
    let length = response_head(&mut out, Status::Unauthorized, 12, false);
    let text = core::str::from_utf8(&out[..length]).unwrap();
    assert_eq!(
        text,
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\
WWW-Authenticate: Bearer realm=\"EggOS\"\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    );
    let length = response_head(&mut out, Status::Accepted, 0, true);
    let text = core::str::from_utf8(&out[..length]).unwrap();
    assert_eq!(
        text,
        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nCache-Control: no-store\r\n\
Connection: keep-alive\r\n\r\n"
    );
    let length = response_head(&mut out, Status::MethodNotAllowed, 5, true);
    assert!(
        core::str::from_utf8(&out[..length])
            .unwrap()
            .contains("Allow: POST\r\n")
    );
}

const MODERN_BODY: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ls","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;

#[test]
fn stateless_messages_must_say_in_their_headers_what_their_body_does() {
    let (request, _) = complete(MCP_POST);
    assert_eq!(check_stateless_headers(&request), Ok(false));
    let modern = Request {
        body: MODERN_BODY,
        protocol_version: Some(b"2026-07-28"),
        mcp_method: Some(b"tools/call"),
        mcp_name: Some(b"ls"),
        ..request
    };
    assert_eq!(check_stateless_headers(&modern), Ok(true));
    let encoded = Request {
        mcp_name: Some(b"=?base64?bHM=?="),
        ..modern
    };
    assert_eq!(check_stateless_headers(&encoded), Ok(true));
    for wrong in [
        Request {
            protocol_version: None,
            ..modern
        },
        Request {
            mcp_method: Some(b"tools/list"),
            ..modern
        },
        Request {
            mcp_name: Some(b"cat"),
            ..modern
        },
    ] {
        assert!(check_stateless_headers(&wrong).is_err());
    }
    let mut out = [0u8; 256];
    let length = header_mismatch_body("Mcp-Name does not match", &mut out);
    let text = core::str::from_utf8(&out[..length]).unwrap();
    assert!(text.contains("\"code\":-32020"), "{text}");
}

#[test]
fn stateless_answers_carry_their_errors_in_the_http_status() {
    assert_eq!(
        stateless_status(br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"x"}}"#),
        Status::NotFound
    );
    assert_eq!(
        stateless_status(br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32022,"message":"x"}}"#),
        Status::BadRequest
    );
    assert_eq!(
        stateless_status(br#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete"}}"#),
        Status::Ok
    );
}
