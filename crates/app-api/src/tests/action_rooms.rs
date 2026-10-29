//! #1978 AC-1: SDK のゲームの action session の room の作成・一覧・終了。

use super::*;
use crate::CreateActionSessionInput;

async fn create_action_room(
    app: &AppService,
    topic: &str,
    title: &str,
    application_id: &str,
) -> String {
    // room の id は作成の時刻(ms)を含むので、同じ ms に作らない。
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    app.create_game_room(
        topic,
        CreateGameRoomInput {
            title: title.into(),
            description: String::new(),
            participants: Vec::new(),
            action: Some(CreateActionSessionInput {
                application_id: application_id.into(),
                max_players: 4,
            }),
        },
    )
    .await
    .expect("create action room")
}

fn ids(rooms: &[GameRoomView]) -> Vec<String> {
    rooms.iter().map(|room| room.room_id.clone()).collect()
}

// #1978 AC-1: action session の room は application ID ごとに更新の新しい順に 20 件ずつ読め、ScoreGame の一覧と
// 他のゲームの一覧には混ざらない。他のゲームの room と ScoreGame が増えても、1 頁は 20 件のまま。
#[tokio::test]
async fn action_rooms_are_listed_per_application_newest_first_in_pages_of_twenty() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:action-rooms";
    let mut arena = Vec::new();
    for index in 0..22 {
        arena.push(
            create_action_room(&app, topic, &format!("arena {index}"), "com.example.arena").await,
        );
        create_action_room(
            &app,
            topic,
            &format!("puzzle {index}"),
            "com.example.puzzle",
        )
        .await;
    }
    let score = app
        .create_game_room(
            topic,
            CreateGameRoomInput {
                title: "score".into(),
                description: String::new(),
                participants: vec!["Alice".into(), "Bob".into()],
                action: None,
            },
        )
        .await
        .expect("create score game");

    let first = app
        .list_action_rooms(topic, TimelineScope::Public, "com.example.arena", None)
        .await
        .expect("first page");
    let newest: Vec<_> = arena.iter().rev().cloned().collect();
    assert_eq!(ids(&first.items), newest[..20]);
    let room = &first.items[0];
    assert_eq!(room.room_kind, GameRoomKind::ActionSession);
    assert_eq!(
        room.action,
        Some(kukuri_core::ActionSessionV1 {
            application_id: "com.example.arena".into(),
            host_endpoint_id: "self".into(),
            max_players: 4,
        })
    );
    let second = app
        .list_action_rooms(
            topic,
            TimelineScope::Public,
            "com.example.arena",
            first.next_cursor.as_deref(),
        )
        .await
        .expect("second page");
    assert_eq!(ids(&second.items), newest[20..]);
    assert_eq!(second.next_cursor, None);
    let scores = app.list_game_rooms(topic).await.expect("score games");
    assert_eq!(ids(&scores), [score]);
}

// #1978 AC-1: host が終えた room は、一覧に「終了」のまま残り、それ以上は更新できない。
#[tokio::test]
async fn an_ended_action_room_stays_listed_as_ended() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:action-room-ended";
    let room_id = create_action_room(&app, topic, "arena", "com.example.arena").await;
    let update = |status| UpdateGameRoomInput {
        status,
        phase_label: None,
        scores: Vec::new(),
    };
    for status in [GameRoomStatus::Running, GameRoomStatus::Ended] {
        app.update_game_room(topic, room_id.as_str(), update(status))
            .await
            .expect("update action room");
    }
    let page = app
        .list_action_rooms(topic, TimelineScope::Public, "com.example.arena", None)
        .await
        .expect("action rooms");
    assert_eq!(ids(&page.items), std::slice::from_ref(&room_id));
    assert_eq!(page.items[0].status, GameRoomStatus::Ended);
    let error = app
        .update_game_room(topic, room_id.as_str(), update(GameRoomStatus::Running))
        .await
        .expect_err("an ended room cannot be reopened");
    assert!(error.to_string().contains("ended"), "{error}");
}

#[tokio::test]
async fn action_room_inputs_are_validated() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:action-room-invalid";
    for (participants, application_id, max_players) in [
        (vec!["Alice".to_string()], "com.example.arena", 4),
        (Vec::new(), "", 4),
        (Vec::new(), "has space", 4),
        (Vec::new(), "com.example.arena", 0),
    ] {
        app.create_game_room(
            topic,
            CreateGameRoomInput {
                title: "arena".into(),
                description: String::new(),
                participants,
                action: Some(CreateActionSessionInput {
                    application_id: application_id.into(),
                    max_players,
                }),
            },
        )
        .await
        .expect_err("invalid action room input");
    }
    assert!(
        app.list_action_rooms(topic, TimelineScope::Public, "com.example.arena", None)
            .await
            .expect("action rooms")
            .items
            .is_empty()
    );
}
