//! BBR code lists (the ones Trigon shows). Unknown codes are returned as `code N`.

pub fn use_text(code: &str) -> &'static str {
    match code {
        "110" => "Stuehus til landbrugsejendom",
        "120" => "Fritliggende enfamiliehus",
        "121" => "Sammenbygget enfamiliehus",
        "122" => "Fritliggende enfamiliehus i tæt-lav bebyggelse",
        "130" => "Række-, kæde- eller dobbelthus",
        "131" => "Række-, kæde- og klyngehus",
        "132" => "Dobbelthus",
        "140" => "Etagebolig, flerfamiliehus",
        "150" => "Kollegium",
        "160" => "Boligbygning til døgninstitution",
        "190" => "Anden bygning til helårsbeboelse",
        "320" => "Kontor, handel, lager",
        "321" => "Kontor",
        "322" => "Detailhandel",
        "420" => "Undervisning og forskning",
        "510" => "Sommerhus",
        _ => "other use (see BBR code list)",
    }
}

pub fn wall_material(code: &str) -> &'static str {
    match code {
        "1" => "brick (mursten)",
        "2" => "lightweight concrete (letbeton)",
        "3" => "fibre cement incl. asbestos",
        "4" => "half-timbering (bindingsværk)",
        "5" => "timber",
        "6" => "concrete elements",
        "8" => "metal",
        "10" => "fibre cement without asbestos",
        "11" => "PVC",
        "12" => "glass",
        "80" => "none",
        _ => "other",
    }
}

pub fn roof_material(code: &str) -> &'static str {
    match code {
        "1" => "built-up felt, low pitch",
        "2" => "roofing felt (tagpap)",
        "3" => "fibre cement incl. asbestos",
        "4" => "concrete tiles",
        "5" => "clay tiles (tegl)",
        "6" => "metal",
        "7" => "thatch",
        "10" => "fibre cement without asbestos",
        "11" => "PVC",
        "12" => "glass",
        "20" => "green roof",
        "80" => "none",
        _ => "other",
    }
}
