# admin-config Specification (delta)

## REMOVED Requirements

### Requirement: Structured config write reconciles values into the existing TOML document
**Reason**: With routes replaced by the model catalog, no form-editable file fields remain; the config file is reduced to process/bootstrap settings (`listen_addr`, `database`, `admin_password`, `retry`) maintained by hand.
**Migration**: Routes are managed as model catalog deployments via `/admin/api/models`; `retry` is edited in the file directly (hot reload applies it).

### Requirement: Structured write preserves untouched masked secrets
**Reason**: The structured write endpoint is removed together with the structured editor; the text-based config API retains masked-sentinel handling for the remaining file secrets.
**Migration**: Use the text-based config read/write endpoints for the remaining file fields.
