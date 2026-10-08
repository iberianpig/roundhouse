# The server side of inertia_rails: the page object that an Inertia.js
# client reads.
#
# The ingest pass (`src/ingest/inertia.rs`) rewrites `render inertia:`
# and `inertia_share` into calls to the methods in this file:
#
#   inertia_begin("widgets/index", "<version>", false, true)
#   inertia_share_application_controller
#   inertia_page.prop_json("widgets", <json>) if inertia_page.eager?("widgets")
#   if inertia_request?
#     render_inertia_json(status: :ok)
#   else
#     render(inertia_root_html(true), layout: "application", status: :ok)
#   end
#
# Every prop arrives as JSON text (the lowering encodes each value as
# `render json:` does), so the page holds only Strings. It has no
# mixed-value Hash and no Proc, which a strict target could not compile.
# A condition around each prop, not a closure, makes it lazy: an
# optional or deferred prop's expression runs only when the request
# asks for that prop.
#
# Partial reloads match top-level keys only; inertia_rails also accepts
# dot paths into nested props.
module ActionController
  class InertiaPage
    attr_reader :version

    def initialize
      @component = +""
      @version = +""
      @encrypt_history = false
      @partial = false
      @only = []
      @except = []
      @keys = []
      @values = []
      @shared_keys = []
      @deferred_groups = []
      @deferred_keys = []
    end

    def start(component, version, encrypt_history, partial_component, partial_data, partial_except)
      @component = component
      @version = version
      @encrypt_history = encrypt_history
      @partial = partial_component == component
      @only = InertiaPage.header_list(partial_data)
      @except = InertiaPage.header_list(partial_except)
      @keys = []
      @values = []
      @shared_keys = []
      @deferred_groups = []
      @deferred_keys = []
      nil
    end

    def self.header_list(value)
      out = []
      parts = value.split(",")
      i = 0
      while i < parts.length
        part = parts[i].strip
        out << part unless part.empty?
        i += 1
      end
      out
    end

    def requested?(key)
      (@only.empty? || @only.include?(key)) && !@except.include?(key)
    end

    # A plain prop: sent on every visit, unless a partial reload excludes it.
    def eager?(key)
      !@partial || requested?(key)
    end

    # `InertiaRails.optional`: sent only on a partial reload that names it.
    def optional?(key)
      @partial && requested?(key)
    end

    # `InertiaRails.defer`: not sent on the first visit. The first visit
    # lists it under `deferredProps`, and the client then requests it.
    def deferred?(key, group)
      unless @partial
        @deferred_groups << group
        @deferred_keys << key
        return false
      end
      requested?(key)
    end

    def shared_eager?(key)
      @shared_keys << key unless @shared_keys.include?(key)
      eager?(key)
    end

    def shared_optional?(key)
      @shared_keys << key unless @shared_keys.include?(key)
      optional?(key)
    end

    def shared_deferred?(key, group)
      @shared_keys << key unless @shared_keys.include?(key)
      deferred?(key, group)
    end

    # A later prop of the same name replaces the earlier one in place,
    # as `shared.merge(props)` does.
    def prop_json(key, json)
      i = @keys.index(key)
      if i.nil?
        @keys << key
        @values << json
      else
        @values[i] = json
      end
      nil
    end

    def to_json(url, notice, alert)
      out = "{\"component\":" + JsonBuilder.encode_value(@component)
      out = out + ",\"props\":{"
      i = 0
      while i < @keys.length
        out = out + "," if i > 0
        out = out + JsonBuilder.encode_value(@keys[i]) + ":" + @values[i]
        i += 1
      end
      out = out + "},\"url\":" + JsonBuilder.encode_value(url)
      out = out + ",\"version\":" + (@version.empty? ? "null" : JsonBuilder.encode_value(@version))
      out = out + ",\"encryptHistory\":" + (@encrypt_history ? "true" : "false")
      out = out + ",\"clearHistory\":false"
      out = out + flash_json(notice, alert)
      out + shared_json + deferred_json + "}"
    end

    def flash_json(notice, alert)
      parts = []
      parts << "\"notice\":" + JsonBuilder.encode_value(notice) unless notice.nil?
      parts << "\"alert\":" + JsonBuilder.encode_value(alert) unless alert.nil?
      return "" if parts.empty?
      ",\"flash\":{" + parts.join(",") + "}"
    end

    def shared_json
      return "" if @shared_keys.empty?
      ",\"sharedProps\":" + InertiaPage.json_string_array(@shared_keys)
    end

    def deferred_json
      return "" if @deferred_keys.empty?
      groups = []
      i = 0
      while i < @deferred_groups.length
        groups << @deferred_groups[i] unless groups.include?(@deferred_groups[i])
        i += 1
      end
      entries = []
      g = 0
      while g < groups.length
        keys = []
        i = 0
        while i < @deferred_keys.length
          keys << @deferred_keys[i] if @deferred_groups[i] == groups[g]
          i += 1
        end
        entries << JsonBuilder.encode_value(groups[g]) + ":" + InertiaPage.json_string_array(keys)
        g += 1
      end
      ",\"deferredProps\":{" + entries.join(",") + "}"
    end

    def self.json_string_array(values)
      parts = []
      i = 0
      while i < values.length
        parts << JsonBuilder.encode_value(values[i])
        i += 1
      end
      "[" + parts.join(",") + "]"
    end
  end

  class Base
    def inertia_page
      page = @inertia_page
      return page unless page.nil?
      fresh = ActionController::InertiaPage.new
      @inertia_page = fresh
      fresh
    end

    def inertia_request?
      req = request
      return false if req.nil?
      req.inertia?
    end

    def inertia_header(name)
      req = request
      return "" if req.nil?
      req.env.fetch(name, "").to_s
    end

    # Starts the page and adds `errors`. The value is the hash that a
    # failed form stored in the session (`redirect_to …, inertia: { errors: … }`),
    # which this render deletes, or `{}` under `always_include_errors_hash`.
    def inertia_begin(component, version, encrypt_history, always_include_errors_hash)
      page = inertia_page
      page.start(component, version, encrypt_history,
                 inertia_header("HTTP_X_INERTIA_PARTIAL_COMPONENT"),
                 inertia_header("HTTP_X_INERTIA_PARTIAL_DATA"),
                 inertia_header("HTTP_X_INERTIA_PARTIAL_EXCEPT"))
      stored = session["inertia_errors"]
      if stored.nil?
        page.prop_json("errors", "{}") if always_include_errors_hash
      else
        session.delete("inertia_errors")
        page.prop_json("errors", stored.to_s)
      end
      nil
    end

    # `redirect_to …, inertia: { errors: e }` stores the errors' JSON in
    # the session for the next Inertia render. It stores JSON text because
    # the session cookie stores each value's `to_s`.
    def inertia_errors_json(json)
      session["inertia_errors"] = json
      nil
    end

    def inertia_url
      req = request
      return "/" if req.nil?
      req.original_fullpath
    end

    # An Inertia visit gets the page object as JSON. A GET whose
    # `X-Inertia-Version` differs from the page's version gets 409 and
    # the URL to reload.
    def render_inertia_json(status: :ok)
      page = inertia_page
      headers["Vary"] = "X-Inertia"
      req = request
      if !req.nil? && req.get? && inertia_header("HTTP_X_INERTIA_VERSION") != page.version
        headers["X-Inertia-Location"] = req.original_url
        head(:conflict)
        return nil
      end
      headers["X-Inertia"] = "true"
      render(page.to_json(inertia_url, flash[:notice], flash[:alert]),
             status: status, content_type: "application/json")
      nil
    end

    # inertia_rails' middleware changes a 302 that answers an Inertia PUT,
    # PATCH or DELETE into a 303, so the client follows it with a GET.
    # The ingest pass installs this method as an after_action on the
    # app's root controller.
    def inertia_redirect_see_other
      req = request
      return nil if req.nil?
      loc = location
      return nil if loc.nil?
      if status == 302 && req.inertia? && ["PUT", "PATCH", "DELETE"].include?(req.request_method)
        redirect_to(loc, status: :see_other)
      end
      nil
    end

    # The page root element for the first visit, rendered inside the app
    # layout. `<`, `>` and `&` in the JSON are escaped as Rails' `to_json`
    # escapes them, so prop text cannot close the script element.
    def inertia_root_html(use_script_element)
      headers["Vary"] = "X-Inertia"
      json = JsonBuilder.escape_html_entities(inertia_page.to_json(inertia_url, flash[:notice], flash[:alert]))
      if use_script_element
        "<script data-page=\"app\" type=\"application/json\">" + json + "</script>\n<div id=\"app\"></div>"
      else
        "<div id=\"app\" data-page=\"" + ActionView::ViewHelpers.html_escape(json) + "\"></div>"
      end
    end
  end
end
