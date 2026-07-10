-- ============================================================================
-- WIM Force Log Out — server setup (run ONCE in the Supabase SQL editor)
-- ============================================================================
-- Corrected to match your real schema:
--   table  devices  (team_id uuid, device_key text, device_paused bool,
--                    last_seen_at, created_at, ...)
--   table  sessions (token text, team_id uuid, staff_id uuid, expires_at, ...)
--   helper session_team_id(token) -> team_id   (already used by your other RPCs)
--
-- Force Log Out sets a "please log out" flag on the device row; the target
-- device sees it on its next heartbeat, logs itself out, and clears the flag.
-- This mirrors your existing set_device_paused / get_device_paused pattern.
--
-- HOW TO RUN:
--   1. Supabase → SQL Editor → clear the box → paste this whole file → Run
--   2. Done. Force Log Out in WIM starts working immediately.
-- Safe to re-run.
-- ============================================================================

-- 1) Flag column on the real devices table.
alter table public.devices
  add column if not exists logout_requested boolean not null default false;

-- 2) Admin asks a specific device (in their own team) to log out.
create or replace function public.request_device_logout(
  p_session_token text,
  p_device_key    text
) returns boolean
language plpgsql
security definer
as $$
declare
  v_team uuid;
begin
  v_team := public.session_team_id(p_token => p_session_token);   -- validates session, returns team_id
  if v_team is null then
    raise exception 'invalid or expired session';
  end if;

  update public.devices
     set logout_requested = true
   where team_id = v_team
     and device_key = p_device_key;

  return true;
end;
$$;

-- 3) A device checks whether it's been asked to log out (called each heartbeat).
create or replace function public.get_device_logout(
  p_session_token text,
  p_device_key    text
) returns boolean
language plpgsql
security definer
as $$
declare
  v_team uuid;
  v_flag boolean;
begin
  v_team := public.session_team_id(p_token => p_session_token);
  if v_team is null then
    return false;
  end if;

  select logout_requested into v_flag
  from public.devices
  where team_id = v_team
    and device_key = p_device_key
  limit 1;

  return coalesce(v_flag, false);
end;
$$;

-- 4) The device clears its own flag right after it logs itself out.
create or replace function public.clear_device_logout(
  p_session_token text,
  p_device_key    text
) returns boolean
language plpgsql
security definer
as $$
declare
  v_team uuid;
begin
  v_team := public.session_team_id(p_token => p_session_token);
  if v_team is null then
    return false;
  end if;

  update public.devices
     set logout_requested = false
   where team_id = v_team
     and device_key = p_device_key;

  return true;
end;
$$;

-- 5) Expose to the app roles (same as your other RPCs).
grant execute on function public.request_device_logout(text, text) to anon, authenticated;
grant execute on function public.get_device_logout(text, text)     to anon, authenticated;
grant execute on function public.clear_device_logout(text, text)   to anon, authenticated;

-- Done.
