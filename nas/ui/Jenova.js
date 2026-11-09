// Jenova: opens a regular DSM window with the Jenova page inside.
Ext.ns("Jenova");

Ext.define("Jenova.AppInstance", {
	extend: "SYNO.SDS.AppInstance",
	appWindowName: "Jenova.AppWindow"
});

Ext.define("Jenova.AppWindow", {
	extend: "SYNO.SDS.AppWindow",

	constructor: function(config) {
		this.appInstance = config.appInstance;
		config = Ext.apply({
			resizable: true,
			maximizable: true,
			minimizable: true,
			width: 1060,
			height: 700,
			minWidth: 880,
			minHeight: 600,
			items: [{
				xtype: "box",
				autoEl: {
					tag: "iframe",
					src: "/webman/3rdparty/Jenova/index.html",
					width: "100%",
					height: "100%",
					frameborder: "0",
					style: "background:#04040c"
				}
			}]
		}, config);
		this.callParent([config]);
	}
});
