-- The modpack a server was set up from with Mod Manager (one per server).
CREATE TABLE IF NOT EXISTS dev_emrekmek_modmanager_modpacks (
    server_uuid UUID PRIMARY KEY REFERENCES servers (uuid) ON DELETE CASCADE,
    provider VARCHAR(16) NOT NULL,
    project_id VARCHAR(64) NOT NULL,
    version_id VARCHAR(64) NOT NULL,
    title VARCHAR(255) NOT NULL,
    version_number VARCHAR(255) NOT NULL,
    installed TIMESTAMPTZ NOT NULL DEFAULT now()
);
