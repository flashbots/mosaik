use super::{announce::Event as AnnounceEvent, sync::Event as SyncEvent};

#[derive(Debug, Clone)]
pub enum Event {
	Announcement(AnnounceEvent),
	CatalogSync(SyncEvent),
}
