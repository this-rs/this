-- Create the links table for LinkService storage.
--
-- Links represent relationships between entities. The source_type and
-- target_type columns are nullable because LinkEntity does not carry
-- this information at the instance level (it lives in LinkDefinition config).
-- When available, they enable efficient type-scoped traversal queries.

CREATE TABLE IF NOT EXISTS links (
    id              UUID            PRIMARY KEY,
    entity_type     VARCHAR(255)    NOT NULL DEFAULT 'link',
    link_type       VARCHAR(255)    NOT NULL,
    source_id       UUID            NOT NULL,
    target_id       UUID            NOT NULL,
    source_type     VARCHAR(255),
    target_type     VARCHAR(255),
    status          VARCHAR(64)     NOT NULL DEFAULT 'active',
    tenant_id       UUID,
    metadata        JSONB           NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    deleted_at      TIMESTAMPTZ
);

-- Index on source_id for find_by_source queries
CREATE INDEX idx_links_source ON links(source_id);

-- Index on target_id for find_by_target queries
CREATE INDEX idx_links_target ON links(target_id);

-- Composite index for find_by_source with link_type filter
CREATE INDEX idx_links_source_link_type ON links(source_id, link_type);

-- Composite index for find_by_target with link_type filter
CREATE INDEX idx_links_target_link_type ON links(target_id, link_type);

-- Composite index for find_by_source with target_type filter
CREATE INDEX idx_links_source_target_type ON links(source_id, target_type)
    WHERE target_type IS NOT NULL;

-- Composite index for find_by_target with source_type filter
CREATE INDEX idx_links_target_source_type ON links(target_id, source_type)
    WHERE source_type IS NOT NULL;

-- Index on tenant_id for multi-tenant isolation
CREATE INDEX idx_links_tenant ON links(tenant_id) WHERE tenant_id IS NOT NULL;
