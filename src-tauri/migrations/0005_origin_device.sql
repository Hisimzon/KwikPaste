-- Device a record was received from over LAN sync. NULL means it was captured on this device.
-- Only locally captured records are offered to peers during catch-up, so received records never bounce back.
ALTER TABLE clipboard_items ADD COLUMN origin_device_id TEXT;
