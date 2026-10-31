use super::server::{Offered, request_type, write_answer};
use super::*;

const MAC: MacAddress = MacAddress([0x52, 0x54, 0, 0x12, 0x34, 0x56]);

fn offered() -> Offered {
    Offered {
        address: Ipv4Address::new(10, 0, 2, 15),
        mask: Ipv4Address::new(255, 255, 255, 0),
        router: Ipv4Address::new(10, 0, 2, 2),
        dns: Ipv4Address::new(10, 0, 2, 3),
        server: Ipv4Address::new(10, 0, 2, 2),
        lease_s: 86_400,
    }
}

fn answer(client: &mut Client, request: &[u8], kind: u8, now_ms: u64) -> Event {
    let mut reply = [0u8; MESSAGE_BYTES];
    let length = write_answer(&mut reply, request, kind, &offered()).unwrap();
    client.receive(&reply[..length], now_ms)
}

fn bound_client() -> Client {
    let mut client = Client::new(MAC, 42);
    let mut message = [0u8; MESSAGE_BYTES];
    let (sent, _) = client.poll(0, &mut message);
    assert!(sent.is_some());
    assert_eq!(
        answer(&mut client, &message, message_type::OFFER, 5),
        Event::Nothing
    );
    let (sent, _) = client.poll(5, &mut message);
    assert!(sent.is_some());
    assert_eq!(
        answer(&mut client, &message, message_type::ACK, 10),
        Event::Bound
    );
    client
}

#[test]
fn discover_offer_request_and_ack_give_the_lease_qemu_offers() {
    let mut client = Client::new(MAC, 42);
    let mut message = [0u8; MESSAGE_BYTES];
    let (sent, event) = client.poll(0, &mut message);
    assert_eq!(event, Event::Nothing);
    assert_eq!(
        sent,
        Some(Outgoing {
            length: MESSAGE_BYTES,
            source: Ipv4Address::UNSPECIFIED,
            destination: Ipv4Address::BROADCAST,
        })
    );
    assert_eq!(request_type(&message), message_type::DISCOVER);
    assert_eq!(&message[28..34], &MAC.0);
    assert_eq!(message[10], 0x80, "servers are asked to broadcast");
    assert_eq!(
        answer(&mut client, &message, message_type::OFFER, 5),
        Event::Nothing
    );
    assert_eq!(client.phase(), Phase::Requesting);
    let (sent, _) = client.poll(5, &mut message);
    assert!(sent.is_some());
    assert_eq!(request_type(&message), message_type::REQUEST);
    let options = &message[OPTIONS_OFFSET..];
    assert!(
        options
            .windows(6)
            .any(|window| window == [option::REQUESTED_ADDRESS, 4, 10, 0, 2, 15])
    );
    assert!(
        options
            .windows(6)
            .any(|window| window == [option::SERVER_ID, 4, 10, 0, 2, 2])
    );
    assert_eq!(
        answer(&mut client, &message, message_type::ACK, 10),
        Event::Bound
    );
    let lease = *client.lease().unwrap();
    assert_eq!(lease.subnet, Subnet::parse(b"10.0.2.15/24").unwrap());
    assert_eq!(lease.router, Some(Ipv4Address::new(10, 0, 2, 2)));
    assert_eq!(lease.dns, [Some(Ipv4Address::new(10, 0, 2, 3)), None, None]);
    assert_eq!(lease.server, Ipv4Address::new(10, 0, 2, 2));
    assert_eq!(
        (lease.lease_s, lease.renewal_s, lease.rebinding_s),
        (86_400, 43_200, 75_600)
    );
    assert_eq!(lease.left_s(10 + 58_000), 86_342);
    assert_eq!(client.phase(), Phase::Bound);
    assert_eq!(client.poll(1000, &mut message), (None, Event::Nothing));
    assert_eq!(client.next_poll_ms(), 10 + 43_200_000);
}

#[test]
fn the_lease_is_renewed_with_its_server_then_rebound_with_any_and_a_nak_loses_it() {
    let mut client = bound_client();
    let mut message = [0u8; MESSAGE_BYTES];
    let renew_at = 10 + 43_200_000;
    let (sent, _) = client.poll(renew_at, &mut message);
    assert_eq!(client.phase(), Phase::Renewing);
    assert_eq!(
        sent,
        Some(Outgoing {
            length: MESSAGE_BYTES,
            source: Ipv4Address::new(10, 0, 2, 15),
            destination: Ipv4Address::new(10, 0, 2, 2),
        })
    );
    assert_eq!(
        &message[12..16],
        &[10, 0, 2, 15],
        "ciaddr is the address kept"
    );
    assert_eq!(message[10], 0, "a renewal is answered by unicast");
    assert_eq!(client.poll(renew_at + 1, &mut message).0, None);
    let rebind_at = 10 + 75_600_000;
    let (sent, _) = client.poll(rebind_at, &mut message);
    assert_eq!(client.phase(), Phase::Rebinding);
    assert_eq!(sent.unwrap().destination, Ipv4Address::BROADCAST);
    assert_eq!(
        answer(&mut client, &message, message_type::ACK, rebind_at),
        Event::Bound
    );
    assert_eq!(client.lease().unwrap().granted_ms, rebind_at);
    let (sent, _) = client.poll(rebind_at + 43_200_000, &mut message);
    assert!(sent.is_some());
    assert_eq!(
        answer(
            &mut client,
            &message,
            message_type::NAK,
            rebind_at + 43_200_001
        ),
        Event::Lost
    );
    assert_eq!(client.lease(), None);
    assert_eq!(client.phase(), Phase::Selecting);
}

#[test]
fn a_lease_nobody_extends_ends_and_the_client_looks_again() {
    let mut client = bound_client();
    let mut message = [0u8; MESSAGE_BYTES];
    let (sent, event) = client.poll(10 + 86_400_000, &mut message);
    assert_eq!(event, Event::Lost);
    assert_eq!(client.lease(), None);
    assert_eq!(
        request_type(&message[..sent.unwrap().length]),
        message_type::DISCOVER
    );
}

#[test]
fn unanswered_discovers_back_off_then_rest_a_minute_and_start_again() {
    let mut client = Client::new(MAC, 7);
    let mut message = [0u8; MESSAGE_BYTES];
    assert!(client.poll(0, &mut message).0.is_some());
    let mut now = 0;
    let mut waits = Vec::new();
    for _ in 1..DISCOVER_ATTEMPTS {
        let due = client.next_poll_ms();
        waits.push(due - now);
        assert_eq!(client.poll(due - 1, &mut message).0, None);
        assert!(client.poll(due, &mut message).0.is_some());
        now = due;
    }
    let due = client.next_poll_ms();
    waits.push(due - now);
    for (wait, base) in waits.iter().zip([4_000, 8_000, 16_000, 32_000]) {
        assert!((base..base + RANDOM_SPREAD_MS).contains(wait), "{waits:?}");
    }
    assert_eq!(client.poll(due, &mut message).0, None);
    assert_eq!(client.phase(), Phase::NoAnswer);
    assert_eq!(client.next_poll_ms(), due + RETRY_AFTER_NO_ANSWER_MS);
    assert!(
        client
            .poll(due + RETRY_AFTER_NO_ANSWER_MS, &mut message)
            .0
            .is_some()
    );
    assert_eq!(client.phase(), Phase::Selecting);
}

#[test]
fn answers_for_another_transaction_or_adapter_are_ignored() {
    let mut client = Client::new(MAC, 3);
    let mut message = [0u8; MESSAGE_BYTES];
    client.poll(0, &mut message);
    let mut other = message;
    other[4] ^= 1;
    assert_eq!(
        answer(&mut client, &other, message_type::OFFER, 1),
        Event::Nothing
    );
    assert_eq!(client.phase(), Phase::Selecting);
    let mut stranger = message;
    stranger[33] ^= 1;
    assert_eq!(
        answer(&mut client, &stranger, message_type::OFFER, 1),
        Event::Nothing
    );
    assert_eq!(client.phase(), Phase::Selecting);
    assert_eq!(
        answer(&mut client, &message, message_type::ACK, 1),
        Event::Nothing
    );
    assert_eq!(
        answer(&mut client, &message, message_type::OFFER, 1),
        Event::Nothing
    );
    assert_eq!(client.phase(), Phase::Requesting);
}

#[test]
fn a_link_that_comes_back_rebinds_the_lease_held() {
    let mut client = bound_client();
    client.restart(2000);
    assert_eq!(client.phase(), Phase::Rebinding);
    let mut message = [0u8; MESSAGE_BYTES];
    let (sent, _) = client.poll(2000, &mut message);
    assert_eq!(sent.unwrap().destination, Ipv4Address::BROADCAST);
    assert_eq!(&message[12..16], &[10, 0, 2, 15]);
    assert_eq!(
        answer(&mut client, &message, message_type::ACK, 2001),
        Event::Bound
    );
}
