//! IFC4 exchange proxies, spatially contained by IfcSite. Public buildingSMART specification;
//! native landscape entities and surveyed terrain semantics are not claimed by these proxies.
use trigon_model::site::{BuiltSiteObject, Intent};

use super::{Frame, Ifc, Placed, Val, gid, refs, string};

impl Ifc<'_> {
    pub(super) fn site_objects(&mut self, objects: &[BuiltSiteObject], site: (usize, Placed)) {
        let mut contained = Vec::new();
        for o in objects {
            let frame = Frame::at([0.; 3]);
            let placed = self.w.place(Some(site.1), frame);
            let items = self.tessellated(&o.parts, &frame);
            let Some(shape) = self.shape("Tessellation", &items, None, "L-Site-Exchange-Proxies") else {
                continue;
            };
            let class = format!("Trigon {} (exchange proxy)", o.class.name());
            let intent = if o.intent == Intent::Proposal { "PROPOSAL: design placement; not surveyed" } else { "Existing: author-supplied source" };
            let id = self.w.add(format!(
                "IFCBUILDINGELEMENTPROXY({},$,{},{},{},#{},#{shape},{},.USERDEFINED.)",
                gid(&o.id),
                string(&o.name),
                string(intent),
                string(&class),
                placed.id,
                string(&o.id)
            ));
            let mut props = vec![("DesignIntent", Val::Label(intent.into())), ("ExchangeClassification", Val::Label(class))];
            {
                let source = &o.source;
                // Portable source parameters are explicitly identified as Trigon JSON, not IFC
                // engineering quantities or a formal landscape classification.
                if let Ok(json) = serde_json::to_string(source) {
                    props.push(("TrigonSourceJSON", Val::Text(json)));
                }
            }
            self.common("Trigon_SiteObject", id, &o.id, Some(o.status), props);
            contained.push(id);
        }
        if !contained.is_empty() {
            self.w.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE({},$,$,$,({}),#{})", gid("site-objects-contains"), refs(&contained), site.0));
        }
    }
}
