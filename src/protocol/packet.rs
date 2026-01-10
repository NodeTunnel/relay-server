use crate::protocol::ids::*;
use crate::protocol::error::ProtocolError;
use crate::protocol::serialize::{push_bool, push_i32, push_string, push_u64, push_vec_room_info, read_bool, read_i32, read_string, read_u64, read_vec_room_info};

#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub join_code: String,
    pub metadata: String,
}

#[derive(Debug, Clone)]
pub enum Packet {
    Authenticate { app_id: String, version: String },
    ClientAuthenticated,
    CreateRoom { is_public: bool, metadata: String },
    ReqRooms,
    GetRooms { rooms: Vec<RoomInfo> },
    UpdateRoom { room_id: String, metadata: String },
    ReqJoin { room_id: String, metadata: String },
    JoinRes { target_id: u64, room_id: String, allowed: bool },
    ConnectedToRoom { room_id: String, peer_id: i32 },
    PeerJoinAttempt { target_id: u64, metadata: String },
    PeerJoinedRoom { peer_id: i32 },
    PeerLeftRoom { peer_id: i32 },
    GameData { from_peer: i32, data: Vec<u8> },
    ForceDisconnect,
    Error { error_code: i32, error_message: String }
}

impl Packet {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() {
            return Err(ProtocolError::EmptyPacket);
        }

        let packet_id = bytes[0];
        let rest = &bytes[1..];

        Ok(match packet_id {
            AUTHENTICATE => {
                let (app_id, r) = read_string(rest)?;
                let (version, _) = read_string(r)?;
                Packet::Authenticate { app_id, version }
            }

            CLIENT_AUTHENTICATED => Packet::ClientAuthenticated,

            CREATE_ROOM => {
                let (is_public, r) = read_bool(rest)?;
                let metadata = match read_string(r) {
                    Ok((name, _)) => {
                        name
                    }
                    Err(_) => {
                        String::new()
                    }
                };

                Packet::CreateRoom { is_public, metadata }
            },

            JOIN_ROOM => {
                let (room_id, r) = read_string(rest)?;
                let (metadata, _) = read_string(r)?;
                Packet::ReqJoin { room_id, metadata }
            }

            CONNECTED_TO_ROOM => {
                let (room_id, r) = read_string(rest)?;
                let (peer_id, _) = read_i32(r)?;
                Packet::ConnectedToRoom { room_id, peer_id }
            }

            PEER_JOIN_ATTEMPT => {
                let (target_id, r) = read_u64(rest)?;
                let (metadata, _) = read_string(r)?;
                Packet::PeerJoinAttempt { target_id, metadata }
            }

            PEER_JOINED => {
                let (peer_id, _) = read_i32(rest)?;
                Packet::PeerJoinedRoom { peer_id }
            }

            PEER_LEFT => {
                let (peer_id, _) = read_i32(rest)?;
                Packet::PeerLeftRoom { peer_id }
            }

            GAME_DATA => {
                let (peer_id, r) = read_i32(rest)?;
                Packet::GameData { from_peer: peer_id, data: r.to_vec() }
            }

            FORCE_DISCONNECT => Packet::ForceDisconnect,

            ERROR_PACKET => {
                let (error_code, r) = read_i32(rest)?;
                let (error_message, _) = read_string(r)?;
                Packet::Error { error_code, error_message }
            }

            REQ_ROOMS => Packet::ReqRooms,

            GET_ROOMS => {
                let (rooms, _) = read_vec_room_info(rest)?;
                Packet::GetRooms { rooms }
            }

            UPDATE_ROOM => {
                let (room_id, r) = read_string(rest)?;
                let (metadata, _) = read_string(r)?;
                Packet::UpdateRoom { room_id, metadata }
            }

            JOIN_RES => {
                let (target_id, r) = read_u64(rest)?;
                let (room_id, r) = read_string(r)?;
                let (allowed, _) = read_bool(r)?;
                Packet::JoinRes { target_id, room_id, allowed }
            }

            _ => return Err(ProtocolError::UnknownPacketType(packet_id))
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        match self {
            Packet::Authenticate { app_id, version } => {
                buf.push(AUTHENTICATE);
                push_string(&mut buf, app_id);
                push_string(&mut buf, version);
            }

            Packet::ClientAuthenticated => {
                buf.push(CLIENT_AUTHENTICATED);
            }

            Packet::CreateRoom { is_public, metadata } => {
                buf.push(CREATE_ROOM);
                push_bool(&mut buf, *is_public);
                push_string(&mut buf, metadata);
            }

            Packet::ReqRooms => {
                buf.push(REQ_ROOMS);
            }

            Packet::GetRooms { rooms } => {
                buf.push(GET_ROOMS);
                push_vec_room_info(&mut buf, rooms);
            }

            Packet::UpdateRoom { room_id, metadata } => {
                buf.push(UPDATE_ROOM);
                push_string(&mut buf, room_id);
                push_string(&mut buf, metadata);
            }

            Packet::ReqJoin { room_id, metadata } => {
                buf.push(JOIN_ROOM);
                push_string(&mut buf, room_id);
                push_string(&mut buf, metadata);
            }

            Packet::JoinRes { target_id, room_id, allowed } => {
                buf.push(JOIN_RES);
                push_u64(&mut buf, *target_id);
                push_string(&mut buf, room_id);
                push_bool(&mut buf, *allowed);
            }

            Packet::ConnectedToRoom { room_id, peer_id } => {
                buf.push(CONNECTED_TO_ROOM);
                push_string(&mut buf, room_id);
                push_i32(&mut buf, *peer_id);
            }

            Packet::PeerJoinAttempt { target_id, metadata } => {
                buf.push(PEER_JOIN_ATTEMPT);
                push_u64(&mut buf, *target_id);
                push_string(&mut buf, metadata);
            }

            Packet::PeerJoinedRoom { peer_id } => {
                buf.push(PEER_JOINED);
                push_i32(&mut buf, *peer_id);
            }

            Packet::PeerLeftRoom { peer_id } => {
                buf.push(PEER_LEFT);
                push_i32(&mut buf, *peer_id);
            }

            Packet::GameData { from_peer: peer_id, data } => {
                buf.push(GAME_DATA);
                push_i32(&mut buf, *peer_id);
                buf.extend(data);
            }

            Packet::ForceDisconnect => {
                buf.push(FORCE_DISCONNECT);
            }

            Packet::Error { error_code, error_message } => {
                buf.push(ERROR_PACKET);
                push_i32(&mut buf, *error_code);
                push_string(&mut buf, error_message);
            }
        }

        buf
    }
}
