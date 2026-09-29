ALTER TABLE queue_items
    ADD COLUMN IF NOT EXISTS assigned_by text REFERENCES dev_identities(principal_id);
