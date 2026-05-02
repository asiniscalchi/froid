-- Add signal generation status tracking directly to daily_reviews
ALTER TABLE daily_reviews ADD COLUMN signals_status TEXT;
ALTER TABLE daily_reviews ADD COLUMN signals_error TEXT;
ALTER TABLE daily_reviews ADD COLUMN signals_model TEXT;
ALTER TABLE daily_reviews ADD COLUMN signals_prompt_version TEXT;
ALTER TABLE daily_reviews ADD COLUMN signals_updated_at TEXT;

-- Index for the signal reconciliation worker
CREATE INDEX idx_daily_reviews_signals_status ON daily_reviews(signals_status) 
WHERE signals_status IS NULL OR signals_status = 'failed';
