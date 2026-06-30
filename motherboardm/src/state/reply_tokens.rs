use hashbrown::HashMap;
use motherboardm_protocol::{ReplyToken, RequestId, TransportError};

use crate::motherboard_device::FileId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingReply {
    pub client_file_id: FileId,
    pub service_file_id: FileId,
    pub request_id: RequestId,
}

pub struct ReplyTokens {
    next: u64,
    pending: HashMap<ReplyToken, PendingReply>,
}

impl ReplyTokens {
    pub fn new() -> Self {
        Self {
            next: 1,
            pending: HashMap::new(),
        }
    }

    pub fn create(
        &mut self,
        client_file_id: FileId,
        service_file_id: FileId,
        request_id: RequestId,
    ) -> ReplyToken {
        let token = self.allocate_unique();
        self.pending.insert(
            token,
            PendingReply {
                client_file_id,
                service_file_id,
                request_id,
            },
        );
        token
    }

    pub fn consume(
        &mut self,
        token: ReplyToken,
        replying_service_file_id: FileId,
    ) -> Result<PendingReply, TransportError> {
        let Some(pending) = self.pending.get(&token).copied() else {
            return Err(TransportError::InvalidReplyToken);
        };

        if pending.service_file_id != replying_service_file_id {
            return Err(TransportError::InvalidReplyToken);
        }

        self.pending.remove(&token);
        Ok(pending)
    }

    pub fn remove_for_file(&mut self, file_id: FileId) {
        self.pending.retain(|_, pending| {
            pending.client_file_id != file_id && pending.service_file_id != file_id
        });
    }

    fn allocate_unique(&mut self) -> ReplyToken {
        loop {
            let token = ReplyToken(self.next);
            self.next = self.next.wrapping_add(1);
            if self.next == 0 {
                self.next = 1;
            }

            if !self.pending.contains_key(&token) {
                return token;
            }
        }
    }
}

impl Default for ReplyTokens {
    fn default() -> Self {
        Self::new()
    }
}
