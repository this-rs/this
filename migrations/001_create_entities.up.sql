-- Create the generic entities table for DataService<T> storage.
--
-- All Data entities share this table, differentiated by entity_type.
-- Common fields are stored as indexed columns; type-specific fields
-- are stored in the JSONB `data` column for flexible schema evolution.

CREATE TABLE IF NOT EXISTS entities (
    id              UUID            PRIMARY KEY,
    entity_type     VARCHAR(255)    NOT NULL,
    name            VARCHAR(512)    NOT NULL,
    status          VARCHAR(64)     NOT NULL DEFAULT 'active',
    tenant_id       UUID,
    data            JSONB           NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    deleted_at      TIMESTAMPTZ
);

-- Index on entity_type for type-scoped queries (list, search)
CREATE INDEX idx_entities_type ON entities(entity_type);

-- Index on tenant_id for multi-tenant isolation (partial: only non-null)
CREATE INDEX idx_entities_tenant ON entities(tenant_id) WHERE tenant_id IS NOT NULL;

-- Composite index for status-filtered queries within a type
CREATE INDEX idx_entities_type_status ON entities(entity_type, status);

-- GIN index on JSONB data for custom field search (supports @>, ?, ?| operators)
CREATE INDEX idx_entities_data ON entities USING GIN(data);
