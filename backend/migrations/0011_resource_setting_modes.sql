ALTER TABLE system_settings
ADD COLUMN resource_modes_json TEXT NOT NULL DEFAULT '{}';
