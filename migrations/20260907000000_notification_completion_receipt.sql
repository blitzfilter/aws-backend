
-- Existing terminal rows deliberately remain without a replay receipt.
ALTER TABLE notification_deliveries
    ADD COLUMN completed_lease_token uuid,
    ADD COLUMN completed_at timestamptz,
    ADD CONSTRAINT notification_deliveries_completion_shape_check CHECK (
        (completed_lease_token IS NULL AND completed_at IS NULL)
        OR (
            completed_lease_token IS NOT NULL
            AND completed_at IS NOT NULL
            AND status <> 'PROCESSING'
        )
    );
