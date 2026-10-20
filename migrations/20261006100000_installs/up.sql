-- Which project/version a jar was installed from, keyed by the jar's SHA-1 so renames
-- (enable/disable) keep the link. Unknown jars are identified by hash lookups instead.
CREATE TABLE IF NOT EXISTS dev_emrekmek_modmanager_installs (
    server_uuid UUID NOT NULL REFERENCES servers (uuid) ON DELETE CASCADE,
    sha1 CHAR(40) NOT NULL,
    provider VARCHAR(16) NOT NULL,
    project_id VARCHAR(64) NOT NULL,
    version_id VARCHAR(64) NOT NULL,
    installed TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_uuid, sha1)
);
