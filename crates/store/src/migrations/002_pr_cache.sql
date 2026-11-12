ALTER TABLE prs ADD COLUMN merge_base TEXT;
-- Full PR detail as last fetched, so a cached PR renders without the network.
ALTER TABLE prs ADD COLUMN detail_json TEXT;
